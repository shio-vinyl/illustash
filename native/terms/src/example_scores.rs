//! 在已归一化的特征上打分。行号随 UUID 排序，同分时按行号取舍。
use half::{f16, slice::HalfFloatSliceExt};
use std::sync::Arc;

pub type Row = usize;

/// 稀疏标签矩阵。每行的区间在 offsets 中，元素是 (列号, 归一化权重)。
pub(crate) struct Tags {
    pub(crate) width: usize,
    pub(crate) offsets: Vec<usize>,
    pub(crate) entries: Vec<(u32, f32)>,
}

/// CCIP 归一化后用半精度保存在内存里，打分时用 f32。
pub(crate) struct Ccip {
    pub(crate) width: usize,
    pub(crate) values: Vec<f16>,
}

pub(crate) enum Vectors {
    Tags(Tags),
    Ccip(Ccip),
    Combined { tags: Arc<Matrix>, ccip: Arc<Matrix>, rows: Vec<(Row, Row)> },
}

/// 只读特征矩阵。例图、排除项与候选均用同一张矩阵的行号。
pub struct Matrix {
    pub(crate) uuids: Vec<String>,
    pub(crate) vectors: Vectors,
}

impl Matrix {
    pub fn row(&self, uuid: &str) -> Option<Row> {
        self.uuids.binary_search_by(|id| id.as_str().cmp(uuid)).ok()
    }

    pub fn uuid(&self, row: Row) -> &str {
        &self.uuids[row]
    }

    pub fn len(&self) -> usize {
        self.uuids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.uuids.is_empty()
    }

    /// 特征内容与顺序一起入版本，识别结果被替换后同一标签的旧证据会退出。
    pub(crate) fn fingerprint(&self) -> sha2::digest::Output<sha2::Sha256> {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update((self.uuids.len() as u64).to_le_bytes());
        for id in &self.uuids {
            crate::example_learning::hash_field(&mut digest, id.as_bytes());
        }
        digest.update((self.width() as u64).to_le_bytes());
        match &self.vectors {
            Vectors::Tags(tags) => {
                for offset in &tags.offsets {
                    digest.update((*offset as u64).to_le_bytes());
                }
                for &(column, score) in &tags.entries {
                    digest.update(column.to_le_bytes());
                    digest.update(score.to_le_bytes());
                }
            }
            Vectors::Ccip(ccip) => {
                for value in &ccip.values {
                    digest.update(value.to_le_bytes());
                }
            }
            Vectors::Combined { tags, ccip, .. } => {
                digest.update(tags.fingerprint());
                digest.update(ccip.fingerprint());
            }
        }
        digest.finalize()
    }

    fn width(&self) -> usize {
        match &self.vectors {
            Vectors::Tags(tags) => tags.width,
            Vectors::Ccip(ccip) => ccip.width,
            Vectors::Combined { tags, ccip, .. } => tags.width() + ccip.width(),
        }
    }

    fn entries(&self, row: Row, apply: &mut dyn FnMut(usize, f32)) {
        match &self.vectors {
            Vectors::Tags(tags) => {
                for &(column, weight) in &tags.entries[tags.offsets[row]..tags.offsets[row + 1]] {
                    apply(column as usize, weight);
                }
            }
            Vectors::Ccip(ccip) => {
                let start = row * ccip.width;
                for (column, &weight) in ccip.values[start..start + ccip.width].iter().enumerate() {
                    apply(column, weight.to_f32());
                }
            }
            Vectors::Combined { tags, ccip, rows } => {
                let (tags_row, ccip_row) = rows[row];
                let scale = std::f32::consts::FRAC_1_SQRT_2;
                tags.entries(tags_row, &mut |col, value| apply(col, value * scale));
                let shift = tags.width();
                ccip.entries(ccip_row, &mut |col, value| apply(shift + col, value * scale));
            }
        }
    }

    fn dot(&self, row: Row, query: &[f32]) -> f32 {
        let score = match &self.vectors {
            Vectors::Tags(tags) => tags.entries[tags.offsets[row]..tags.offsets[row + 1]]
                .iter()
                .map(|&(column, value)| value * query[column as usize])
                .sum(),
            Vectors::Ccip(ccip) => {
                let start = row * ccip.width;
                let values = &ccip.values[start..start + ccip.width];
                // 批量转换在有硬件支持的主机上走向量指令。
                let mut scratch = [0.0; 128];
                values.chunks(128).zip(query.chunks(128)).fold(0.0, |score, (v, q)| {
                    let converted = &mut scratch[..v.len()];
                    v.convert_to_f32_slice(converted);
                    score + converted.iter().zip(q).map(|(value, query)| value * query).sum::<f32>()
                })
            }
            Vectors::Combined { tags, ccip, rows } => {
                let (tag_query, ccip_query) = query.split_at(tags.width());
                let (tag_row, ccip_row) = rows[row];
                (tags.dot(tag_row, tag_query) + ccip.dot(ccip_row, ccip_query)) * std::f32::consts::FRAC_1_SQRT_2
            }
        };
        // 让正零与负零进入同一个分数档。
        score + 0.0
    }
}

/// 正例、人工负例、训练背景和不应出现在结果中的行。
#[derive(Default)]
pub struct Examples<'a> {
    pub positives: &'a [Row],
    pub negatives: &'a [Row],
    pub background: &'a [Row],
    pub exclude: &'a [Row],
}

#[derive(Clone, Copy)]
pub enum Scorer {
    Centroid,
    MaxSim,
    Linear { l2: f32 },
}

fn direction(v: &mut [f32]) {
    let norm = v.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>().sqrt() as f32;
    if norm > 1e-12 {
        for x in v {
            *x /= norm;
        }
    } else {
        v.fill(0.0);
    }
}

/// 按分数降序返回未排除的所有行。
pub fn score(m: &Matrix, ex: &Examples<'_>, scorer: &Scorer) -> Vec<(Row, f32)> {
    let candidates: Vec<_> = (0..m.len()).collect();
    top(m, ex, scorer, &candidates, candidates.len())
}

/// 只在给定候选中排序，最多返回 k 行。没有可用正例时返回空列表。
pub fn top(m: &Matrix, ex: &Examples<'_>, scorer: &Scorer, candidates: &[Row], k: usize) -> Vec<(Row, f32)> {
    if ex.positives.is_empty() || k == 0 {
        return vec![];
    }
    let (queries, bias) = match *scorer {
        Scorer::Centroid => {
            let mut centre = vec![0.0; m.width()];
            for &row in ex.positives {
                m.entries(row, &mut |col, value| centre[col] += value);
            }
            direction(&mut centre);
            (vec![centre], 0.0)
        }
        Scorer::MaxSim => (
            ex.positives
                .iter()
                .map(|&row| {
                    let mut query = vec![0.0; m.width()];
                    m.entries(row, &mut |col, value| query[col] = value);
                    query
                })
                .collect(),
            0.0,
        ),
        Scorer::Linear { l2 } => {
            let Some((weights, bias)) = linear(m, ex, l2) else {
                return vec![];
            };
            (vec![weights], bias)
        }
    };
    let mut excluded = vec![false; m.len()];
    for &row in ex.exclude {
        excluded[row] = true;
    }
    let evaluate = |rows: &[Row]| {
        rows.iter()
            .copied()
            .filter(|&row| !excluded[row])
            .map(|row| {
                let value = queries.iter().map(|q| m.dot(row, q)).fold(f32::NEG_INFINITY, f32::max);
                (row, value + bias)
            })
            .collect::<Vec<_>>()
    };
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get()).min(4);
    let chunk_size = candidates.len().div_ceil(cpus).max(4096);
    let mut ranked = if candidates.len() <= chunk_size {
        evaluate(candidates)
    } else {
        std::thread::scope(|scope| {
            let mut chunks = candidates.chunks(chunk_size);
            let first = chunks.next().expect("large candidate set has a first chunk");
            let workers: Vec<_> = chunks.map(|rows| scope.spawn(|| evaluate(rows))).collect();
            let mut ranked = evaluate(first);
            for worker in workers {
                ranked.extend(worker.join().expect("example scorer must not panic"));
            }
            ranked
        })
    };
    let compare = |a: &(Row, f32), b: &(Row, f32)| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0));
    if k < ranked.len() {
        ranked.select_nth_unstable_by(k, compare);
        ranked.truncate(k);
    }
    ranked.sort_unstable_by(compare);
    ranked
}

fn linear(m: &Matrix, ex: &Examples<'_>, l2: f32) -> Option<(Vec<f32>, f32)> {
    let negative_count = ex.negatives.len() + ex.background.len();
    if negative_count == 0 || !l2.is_finite() || l2 <= 0.0 {
        return None;
    }
    let width = m.width();
    let scale = f64::from(l2) / (ex.positives.len() + negative_count) as f64;
    let positive_weight = 0.5 / ex.positives.len() as f64;
    let negative_weight = 0.5 / negative_count as f64;
    let examples = ex
        .positives
        .iter()
        .map(|&row| (row, true, positive_weight))
        .chain(ex.negatives.iter().chain(ex.background).map(|&row| (row, false, negative_weight)));
    let evaluate = |p: &[f64]| {
        let mut gradient: Vec<_> = p[..width].iter().map(|x| x * scale).collect();
        gradient.push(0.0);
        let mut loss = p[..width].iter().map(|x| x * x).sum::<f64>() * scale * 0.5;
        for (row, positive, weight) in examples.clone() {
            let mut logit = p[width];
            m.entries(row, &mut |col, value| logit += f64::from(value) * p[col]);
            let signed = if positive { -logit } else { logit };
            loss += weight * (signed.max(0.0) + (-signed.abs()).exp().ln_1p());
            let residual = weight * (1.0 / (1.0 + (-logit).exp()) - f64::from(u8::from(positive)));
            gradient[width] += residual;
            m.entries(row, &mut |col, value| gradient[col] += residual * f64::from(value));
        }
        (loss, gradient)
    };
    let params = crate::research_compute::discriminate::fit_single(width, evaluate);
    Some((params[..width].iter().map(|&x| x as f32).collect(), params[width] as f32))
}
