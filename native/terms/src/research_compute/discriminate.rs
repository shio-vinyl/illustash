//! 原型之外的判别式强认领（#156，docs/TERMS_CHARACTERS.md）。
//!
//! 每个至少有 `MIN_POSITIVES` 张确认图的角色训练一个 L2 逻辑回归。
//! 负例是该角色被移出的图、其他角色的确认图，以及外部索引只归给其他角色的未裁决图；后两类跳过含有该角色
//! 自己正例的作品组。认领还要求图片离某张确认图在 `GATE` 以内，并且离它比离任何被移出的图都近。
//! 没有外部索引候选时这个方法不启用：它只认得用户标注过的角色，会更频繁地认领没见过的相似角色。
//!
//! 所有角色一起训练：行是共享的，每次求值是经研究计算 BLAS 的两次乘积，每个角色保留自己的 L-BFGS 记忆。
use super::{features::Matrix, numeric};
use crate::{Error, Result};
use illustash_platform::platform_research_math::cross;
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const MIN_POSITIVES: usize = 5;
/// L2 强度的倒数，与 scikit-learn 的 `C` 相同，作用在按权重归一化的目标函数上。
const C: f64 = 10.0;
const TAU: f64 = 0.99;
const GATE: f32 = 0.06;
const MEMORY: usize = 10;
const GRADIENT_TOLERANCE: f64 = 1e-5;
const DECREASE_TOLERANCE: f64 = 1e-9;
const MAX_ITERATIONS: usize = 300;
const BACKTRACKS: usize = 20;
const ARMIJO: f64 = 1e-4;

/// 共享的训练行：`x` 是行主序的 n x d，`xt` 是它的转置。
struct Rows {
    n: usize,
    d: usize,
    x: Vec<f32>,
    xt: Vec<f32>,
}

/// 每个角色的标签和均衡权重（行主序 n x e）。
struct Labels {
    e: usize,
    y: Vec<f32>,
    w: Vec<f32>,
    total: Vec<f64>,
}

/// 每个角色一次被接受的 L-BFGS 步：(s, y, 1 / s.y)。
type Memory = Vec<(Vec<f64>, Vec<f64>, f64)>;

struct Objective {
    f: Vec<f64>,
    g: Vec<Vec<f64>>,
}

/// 在参数 `p`（每个角色一个 d+1 向量，偏置在最后）处，对 `which` 里的角色求按权重归一化的损失和梯度。
fn evaluate(rows: &Rows, labels: &Labels, which: &[usize], p: &[Vec<f64>]) -> Result<Objective> {
    let (n, d, k) = (rows.n, rows.d, which.len());
    let weights: Vec<f32> = p.iter().flat_map(|v| v[..d].iter().map(|&x| x as f32)).collect();
    let z = cross(&rows.x, n, &weights, k, d).map_err(Error::invalid)?;
    let mut f = vec![0.0f64; k];
    let mut r = vec![0.0f32; k * n]; // k x n，转置后的残差
    let mut gb = vec![0.0f64; k];
    for i in 0..n {
        for (c, &j) in which.iter().enumerate() {
            let w = labels.w[i * labels.e + j] as f64;
            if w == 0.0 {
                continue;
            }
            let y = labels.y[i * labels.e + j] as f64;
            let zz = (z[i * k + c] as f64 + p[c][d]).clamp(-30.0, 30.0);
            let s = if y > 0.0 { -zz } else { zz };
            f[c] += w * (s.max(0.0) + (-s.abs()).exp().ln_1p());
            let res = w * (1.0 / (1.0 + (-zz).exp()) - y) / labels.total[j];
            r[c * n + i] = res as f32;
            gb[c] += res;
        }
    }
    let gw = cross(&r, k, &rows.xt, d, n).map_err(Error::invalid)?;
    let mut g = Vec::with_capacity(k);
    for (c, &j) in which.iter().enumerate() {
        let scale = C * labels.total[j];
        let norm: f64 = p[c][..d].iter().map(|v| v * v).sum();
        f[c] = f[c] / labels.total[j] + 0.5 * norm / scale;
        let mut gc: Vec<f64> = (0..d).map(|t| gw[c * d + t] as f64 + p[c][t] / scale).collect();
        gc.push(gb[c]);
        g.push(gc);
    }
    Ok(Objective { f, g })
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// 从零开始的批量 L-BFGS；每个角色返回一个 d+1 的参数向量。
fn fit(rows: &Rows, labels: &Labels) -> Result<Vec<Vec<f64>>> {
    fit_with(rows.d, labels.e, MAX_ITERATIONS, GRADIENT_TOLERANCE, |which, p| evaluate(rows, labels, which, p))
}

/// 例图线性头使用同一优化器，目标函数直接扫稀疏特征。
pub(crate) fn fit_single(width: usize, evaluate: impl Fn(&[f64]) -> (f64, Vec<f64>)) -> Vec<f64> {
    // 例图离线用 sklearn 的类均衡头，max_iter=2000，默认 tol=1e-4。
    fit_with(width, 1, 2000, 1e-4, |_, p| {
        let (f, g) = evaluate(&p[0]);
        Ok::<_, std::convert::Infallible>(Objective { f: vec![f], g: vec![g] })
    })
    .unwrap_or_else(|never| match never {})
    .pop()
    .expect("one fitted example head")
}

fn fit_with<E>(
    width: usize,
    e: usize,
    max_iterations: usize,
    gradient_tolerance: f64,
    evaluate: impl Fn(&[usize], &[Vec<f64>]) -> std::result::Result<Objective, E>,
) -> std::result::Result<Vec<Vec<f64>>, E> {
    let all: Vec<usize> = (0..e).collect();
    let mut x = vec![vec![0.0f64; width + 1]; e];
    let start = evaluate(&all, &x)?;
    let (mut f, mut g) = (start.f, start.g);
    let mut history: Vec<Memory> = vec![vec![]; e];
    let mut active = vec![true; e];
    for _ in 0..max_iterations {
        for j in 0..e {
            active[j] &= g[j].iter().fold(0.0f64, |m, v| m.max(v.abs())) > gradient_tolerance;
        }
        let pending: Vec<usize> = (0..e).filter(|&j| active[j]).collect();
        if pending.is_empty() {
            break;
        }
        let mut direction = HashMap::new();
        for &j in &pending {
            let mut q = g[j].clone();
            let mut alphas = vec![];
            for (s, y, rho) in history[j].iter().rev() {
                let a = rho * dot(s, &q);
                alphas.push(a);
                q.iter_mut().zip(y).for_each(|(q, y)| *q -= a * y);
            }
            if let Some((s, y, _)) = history[j].last() {
                let gamma = dot(s, y) / dot(y, y);
                q.iter_mut().for_each(|q| *q *= gamma);
            }
            for ((s, y, rho), a) in history[j].iter().zip(alphas.iter().rev()) {
                let b = rho * dot(y, &q);
                q.iter_mut().zip(s).for_each(|(q, s)| *q += s * (a - b));
            }
            q.iter_mut().for_each(|q| *q = -*q);
            direction.insert(j, q);
        }
        let mut step: HashMap<usize, f64> = pending.iter().map(|&j| (j, 1.0)).collect();
        let mut waiting = pending.clone();
        for attempt in 0..BACKTRACKS {
            let trial: Vec<Vec<f64>> = waiting
                .iter()
                .map(|j| x[*j].iter().zip(&direction[j]).map(|(x, d)| x + step[j] * d).collect())
                .collect();
            let next = evaluate(&waiting, &trial)?;
            let mut still = vec![];
            for (c, &j) in waiting.iter().enumerate() {
                let slope = dot(&g[j], &direction[&j]);
                if next.f[c] <= f[j] + ARMIJO * step[&j] * slope {
                    if f[j] - next.f[c] <= DECREASE_TOLERANCE * f[j].abs().max(next.f[c].abs()).max(1.0) {
                        active[j] = false;
                    }
                    let s: Vec<f64> = trial[c].iter().zip(&x[j]).map(|(a, b)| a - b).collect();
                    let y: Vec<f64> = next.g[c].iter().zip(&g[j]).map(|(a, b)| a - b).collect();
                    let sy = dot(&s, &y);
                    if sy > 1e-10 {
                        history[j].push((s, y, 1.0 / sy));
                        if history[j].len() > MEMORY {
                            history[j].remove(0);
                        }
                    }
                    x[j] = trial[c].clone();
                    f[j] = next.f[c];
                    g[j] = next.g[c].clone();
                } else {
                    still.push(j);
                }
            }
            if still.is_empty() {
                break;
            }
            if attempt + 1 == BACKTRACKS {
                // 在 float32 精度下已无法再下降：保留当前参数。
                for j in still {
                    active[j] = false;
                }
                break;
            }
            for j in &still {
                *step.get_mut(j).expect("pending step") *= 0.5;
            }
            waiting = still;
        }
    }
    Ok(x)
}

fn strings(c: &Connection, sql: &str) -> Result<Vec<(String, String)>> {
    Ok(c.prepare(sql)?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
}

/// 强认领 `(uuid, entity) -> 1 - distance to the nearest confirmed image`。
pub fn claims(
    c: &Connection,
    f: &Matrix,
    marks: &BTreeSet<String>,
    groups: &HashMap<String, String>,
    assets: &[String],
) -> Result<BTreeMap<(String, String), f64>> {
    let table: bool =
        c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='candidates')", [], |r| r.get(0))?;
    if !table || !c.query_row("SELECT EXISTS(SELECT 1 FROM candidates WHERE source='CCIP')", [], |r| r.get(0))? {
        return Ok(BTreeMap::new());
    }
    let usable = |u: &String| f.row.contains_key(u) && groups.contains_key(u);
    let mut confirmed = BTreeMap::<String, BTreeSet<String>>::new();
    let mut judged = BTreeSet::new();
    for (u, e) in strings(c, "SELECT uuid,entity FROM family_confirmed")? {
        if marks.contains(&e) {
            continue;
        }
        judged.insert(u.clone());
        if usable(&u) {
            confirmed.entry(e).or_default().insert(u);
        }
    }
    let mut rejected = BTreeMap::<String, BTreeSet<String>>::new();
    for (u, e) in
        strings(c, "SELECT s.uuid,s.entity FROM assertions s JOIN assets a ON a.uuid=s.uuid WHERE s.status='rejected'")?
    {
        judged.insert(u.clone());
        if usable(&u) && !confirmed.get(&e).is_some_and(|p| p.contains(&u)) {
            rejected.entry(e).or_default().insert(u);
        }
    }
    let mut pseudo = BTreeMap::<String, BTreeSet<String>>::new();
    // 可撤回的扩展头不提供伪标签，撤回后不会留下用它选取的训练负例。
    for (u, e) in
        strings(c, "SELECT DISTINCT uuid,entity FROM candidates WHERE source NOT IN ('WD_EXT','CCIP_LONGTAIL')")?
    {
        if !judged.contains(&u) && usable(&u) {
            pseudo.entry(u).or_default().insert(e);
        }
    }
    let trained: Vec<String> =
        confirmed.iter().filter(|(_, p)| p.len() >= MIN_POSITIVES).map(|(e, _)| e.clone()).collect();
    if trained.is_empty() {
        return Ok(BTreeMap::new());
    }
    let confirmed_any: BTreeSet<&String> = confirmed.values().flatten().collect();
    let ids: BTreeSet<&String> =
        confirmed_any.iter().copied().chain(rejected.values().flatten()).chain(pseudo.keys()).collect();
    let ids: Vec<&String> = ids.into_iter().collect();
    let (n, d, e) = (ids.len(), f.dimension, trained.len());
    let mut x = Vec::with_capacity(n * d);
    for u in &ids {
        x.extend_from_slice(&f.rows[f.row[*u]]);
    }
    let mut xt = vec![0.0f32; n * d];
    for i in 0..n {
        for t in 0..d {
            xt[t * n + i] = x[i * d + t];
        }
    }
    let rows = Rows { n, d, x, xt };
    let mut y = vec![0.0f32; n * e];
    let mut w = vec![0.0f32; n * e];
    for (j, entity) in trained.iter().enumerate() {
        let positives = &confirmed[entity];
        let own: BTreeSet<&String> = positives.iter().map(|u| &groups[u]).collect();
        for (i, u) in ids.iter().enumerate() {
            let at = i * e + j;
            if positives.contains(*u) {
                y[at] = 1.0;
                w[at] = 1.0;
            } else if rejected.get(entity).is_some_and(|r| r.contains(*u)) {
                w[at] = 1.0;
            } else if own.contains(&groups[*u]) {
                continue;
            } else if confirmed_any.contains(u) || pseudo.get(*u).is_some_and(|c| !c.contains(entity)) {
                w[at] = 1.0;
            }
        }
    }
    // 在每个角色已标注的行上按 scikit-learn 的 balanced 方式计算类别权重。
    let mut total = vec![0.0f64; e];
    for j in 0..e {
        let (mut pos, mut neg) = (0.0f64, 0.0f64);
        for i in 0..n {
            if w[i * e + j] > 0.0 {
                if y[i * e + j] > 0.0 {
                    pos += 1.0
                } else {
                    neg += 1.0
                }
            }
        }
        let all = pos + neg;
        for i in 0..n {
            let at = i * e + j;
            if w[at] > 0.0 {
                w[at] = (all / (2.0 * if y[at] > 0.0 { pos } else { neg.max(1.0) })) as f32;
                total[j] += w[at] as f64;
            }
        }
    }
    let labels = Labels { e, y, w, total };
    let params = fit(&rows, &labels)?;

    let (known, images) = f.matrix(assets);
    let flat: Vec<f32> = images.iter().flatten().copied().collect();
    let weights: Vec<f32> = params.iter().flat_map(|p| p[..d].iter().map(|&v| v as f32)).collect();
    let mut out = BTreeMap::new();
    for block in (0..known.len()).step_by(8192) {
        let end = (block + 8192).min(known.len());
        let z = cross(&flat[block * d..end * d], end - block, &weights, e, d).map_err(Error::invalid)?;
        for (j, entity) in trained.iter().enumerate() {
            let hits: Vec<usize> = (block..end)
                .filter(|&i| {
                    let zz = (z[(i - block) * e + j] as f64 + params[j][d]).clamp(-30.0, 30.0);
                    1.0 / (1.0 + (-zz).exp()) >= TAU
                })
                .collect();
            if hits.is_empty() {
                continue;
            }
            let query: Vec<Vec<f32>> = hits.iter().map(|&i| images[i].clone()).collect();
            let positives: Vec<Vec<f32>> = confirmed[entity].iter().map(|u| f.rows[f.row[u]].clone()).collect();
            let near = numeric::distances(&query, &positives).map_err(Error::invalid)?;
            let removed: Vec<Vec<f32>> =
                rejected.get(entity).map(|r| r.iter().map(|u| f.rows[f.row[u]].clone()).collect()).unwrap_or_default();
            let away = numeric::distances(&query, &removed).map_err(Error::invalid)?;
            for (h, &i) in hits.iter().enumerate() {
                let d_pos = near[h].iter().copied().fold(f32::INFINITY, f32::min);
                let d_neg = away[h].iter().copied().fold(f32::INFINITY, f32::min);
                if d_pos <= GATE && d_pos < d_neg {
                    // 确认图离它最近的就是它自己；f32 舍入可能让这个距离为负。
                    out.insert((known[i].clone(), entity.clone()), 1.0 - d_pos.max(0.0) as f64);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    /// 24 x 4 的合成问题，旧的 Python 参考实现（batch_lr.py，#9 已删除）解过同一个问题。
    fn synthetic() -> (Rows, Labels) {
        let (n, d) = (24, 4);
        let x: Vec<f32> = (0..n)
            .flat_map(|i| {
                unit(
                    &(0..d)
                        .map(|t| (0.7 * i as f32 + t as f32).sin() + if i % 3 == 0 && t == 0 { 1.5 } else { 0.0 })
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let mut xt = vec![0.0; n * d];
        for i in 0..n {
            for t in 0..d {
                xt[t * n + i] = x[i * d + t];
            }
        }
        let mut y = vec![0.0f32; n * 2];
        let mut w = vec![1.0f32; n * 2];
        for i in 0..n {
            y[i * 2] = (i % 3 == 0) as u8 as f32;
            y[i * 2 + 1] = (i % 4 == 1) as u8 as f32;
        }
        w[2 * 2 + 1] = 0.0;
        w[5 * 2 + 1] = 0.0;
        let mut total = vec![0.0; 2];
        for j in 0..2 {
            let pos = (0..n).filter(|&i| w[i * 2 + j] > 0.0 && y[i * 2 + j] > 0.0).count() as f64;
            let all = (0..n).filter(|&i| w[i * 2 + j] > 0.0).count() as f64;
            for i in 0..n {
                let at = i * 2 + j;
                if w[at] > 0.0 {
                    w[at] = (all / (2.0 * if y[at] > 0.0 { pos } else { all - pos })) as f32;
                    total[j] += w[at] as f64;
                }
            }
        }
        (Rows { n, d, x, xt }, Labels { e: 2, y, w, total })
    }

    #[test]
    fn gradient_matches_finite_differences() {
        let (rows, labels) = synthetic();
        let p = vec![vec![0.3, -0.2, 0.5, 0.1, -0.4], vec![-0.1, 0.4, 0.2, -0.3, 0.2]];
        let base = evaluate(&rows, &labels, &[0, 1], &p).unwrap();
        for j in 0..2 {
            for t in 0..5 {
                let h = 1e-3;
                let mut up = p.clone();
                up[j][t] += h;
                let mut down = p.clone();
                down[j][t] -= h;
                let numeric = (evaluate(&rows, &labels, &[j], &[up[j].clone()]).unwrap().f[0]
                    - evaluate(&rows, &labels, &[j], &[down[j].clone()]).unwrap().f[0])
                    / (2.0 * h);
                assert!((numeric - base.g[j][t]).abs() < 1e-4, "{j} {t}: {numeric} vs {}", base.g[j][t]);
            }
        }
    }

    #[test]
    fn fit_matches_python_reference() {
        let (rows, labels) = synthetic();
        let p = fit(&rows, &labels).unwrap();
        let expected =
            [[6.089648, -1.68802, 1.155874, 2.937062, -2.445799], [0.018851, 0.738608, 0.222815, -0.497833, -0.084115]];
        for j in 0..2 {
            for t in 0..5 {
                assert!((p[j][t] - expected[j][t]).abs() < 2e-3, "{j} {t}: {} vs {}", p[j][t], expected[j][t]);
            }
        }
    }

    /// 圆上两个已标注的角色，以及索引归给 `B` 的未裁决图。
    fn library(ccip: bool) -> (Connection, Matrix, HashMap<String, String>) {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE assets(uuid TEXT PRIMARY KEY);
             CREATE TABLE family_confirmed(uuid TEXT, entity TEXT);
             CREATE TABLE assertions(uuid TEXT, entity TEXT, status TEXT);
             CREATE TABLE candidates(uuid TEXT, entity TEXT, source TEXT, score REAL);",
        )
        .unwrap();
        let mut uuids = vec![];
        let mut rows = vec![];
        let mut add = |name: String, angle: f32| {
            c.execute("INSERT INTO assets VALUES (?)", [&name]).unwrap();
            uuids.push(name);
            rows.push(vec![angle.cos(), angle.sin(), 0.05]);
        };
        for i in 0..400 {
            add(format!("A{i}"), 0.00035 * i as f32);
            add(format!("B{i}"), 1.4 + 0.00035 * i as f32);
        }
        for i in 0..300 {
            add(format!("U{i}"), 1.2 + 0.0005 * i as f32);
        }
        add("near".into(), 0.07);
        add("far".into(), -0.6);
        add("removed".into(), 0.35);
        add("closer".into(), 0.34);
        for i in 0..400 {
            c.execute("INSERT INTO family_confirmed VALUES (?, 'A')", [format!("A{i}")]).unwrap();
            c.execute("INSERT INTO family_confirmed VALUES (?, 'B')", [format!("B{i}")]).unwrap();
        }
        c.execute("INSERT INTO assertions VALUES ('removed', 'A', 'rejected')", []).unwrap();
        let source = if ccip { "CCIP" } else { "WD" };
        for i in 0..300 {
            c.execute("INSERT INTO candidates VALUES (?, 'B', ?, 0.9)", [format!("U{i}"), source.into()]).unwrap();
        }
        let groups = uuids.iter().map(|u| (u.clone(), u.clone())).collect();
        (c, Matrix::new(uuids, rows, 3).unwrap(), groups)
    }

    #[test]
    fn claims_need_index_gate_and_distance_from_removed() {
        let (c, f, groups) = library(true);
        let out = claims(&c, &f, &BTreeSet::new(), &groups, &f.uuids).unwrap();
        c.execute("INSERT INTO candidates VALUES ('U1', 'A', 'WD_EXT', 0.99),('far', 'B', 'WD_EXT', 0.99)", [])
            .unwrap();
        assert_eq!(claims(&c, &f, &BTreeSet::new(), &groups, &f.uuids).unwrap(), out);
        let a: BTreeSet<_> = out.keys().filter(|(_, e)| e == "A").map(|(u, _)| u.as_str()).collect();
        assert!(a.contains("near"), "{a:?}");
        assert!(!a.contains("far"), "outside the gate: {a:?}");
        assert!(!a.contains("removed") && !a.contains("closer"), "nearer a removed image: {a:?}");
        assert!(out.keys().all(|(u, e)| !u.starts_with('U') || e == "B"), "index says B: {out:?}");
        // 确认图也会被认领；舍入不能让它们的分数超过 1（Core 会拒绝）。
        assert!(out.values().all(|s| (0.0..=1.0).contains(s)), "{out:?}");
        let score = out[&("near".to_string(), "A".to_string())];
        let a_rows: Vec<_> = (0..400).map(|i| f.rows[f.row[&format!("A{i}")]].clone()).collect();
        let d = numeric::distances(&[f.rows[f.row["near"]].clone()], &a_rows).unwrap()[0]
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min);
        assert_eq!(score, 1.0 - d as f64);
        let (c, f, groups) = library(false);
        assert!(claims(&c, &f, &BTreeSet::new(), &groups, &f.uuids).unwrap().is_empty());
        let marks = BTreeSet::from(["A".to_string(), "B".to_string()]);
        let (c, f, groups) = library(true);
        assert!(claims(&c, &f, &marks, &groups, &f.uuids).unwrap().is_empty());
    }
}
