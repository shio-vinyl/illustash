//! 候选分组和共享 OC 分组。现有的外观阈值和过滤条件不变。
use super::{
    features::Matrix,
    numeric,
    snapshot::{self, Snapshot},
};
use crate::{sql_fragments::first_leads, Error, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const HAIR: &[(&str, &str)] = &[
    ("blonde_hair", "blonde"),
    ("brown_hair", "brown"),
    ("light_brown_hair", "brown"),
    ("black_hair", "black"),
    ("white_hair", "white"),
    ("grey_hair", "white"),
    ("pink_hair", "pink"),
    ("red_hair", "red"),
    ("blue_hair", "blue"),
    ("light_blue_hair", "blue"),
    ("dark_blue_hair", "blue"),
    ("aqua_hair", "blue"),
    ("green_hair", "green"),
    ("purple_hair", "purple"),
    ("orange_hair", "orange"),
];
const OTHER_TRAITS: &[&str] = &[
    "blue_eyes",
    "red_eyes",
    "brown_eyes",
    "green_eyes",
    "purple_eyes",
    "yellow_eyes",
    "pink_eyes",
    "black_eyes",
    "aqua_eyes",
    "grey_eyes",
    "orange_eyes",
    "halo",
    "mechanical_halo",
    "animal_ears",
    "cat_ears",
    "fox_ears",
    "rabbit_ears",
    "dog_ears",
    "wolf_ears",
    "horse_ears",
    "horns",
    "demon_horns",
    "dragon_horns",
    "pointy_ears",
    "tail",
    "wings",
    "ahoge",
    "twintails",
    "ponytail",
    "side_ponytail",
    "braid",
    "hair_bun",
    "drill_hair",
    "hair_over_one_eye",
    "multicolored_hair",
    "streaked_hair",
    "two-tone_hair",
    "gradient_hair",
    "heterochromia",
    "x_hair_ornament",
    "dark_skin",
    "glasses",
    "very_long_hair",
    "short_hair",
];
type Tags = HashMap<String, BTreeSet<String>>;
fn traits() -> Vec<&'static str> {
    HAIR.iter().map(|(t, _)| *t).chain(OTHER_TRAITS.iter().copied()).collect()
}
fn tags(c: &Connection) -> Result<Tags> {
    let allowed: BTreeSet<_> = traits().into_iter().collect();
    let mut tags = Tags::new();
    for row in c
        .prepare("SELECT uuid,value FROM general_hits")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
    {
        let (u, t) = row?;
        if allowed.contains(t.as_str()) {
            tags.entry(u).or_default().insert(t);
        }
    }
    Ok(tags)
}
fn visible(alias: &str) -> String {
    format!("NOT EXISTS(SELECT 1 FROM duplicates h WHERE h.uuid={alias}.uuid AND h.hidden=1) AND NOT EXISTS(SELECT 1 FROM excluded x WHERE x.uuid={alias}.uuid)")
}
fn hair(tags: Option<&BTreeSet<String>>) -> Option<&'static str> {
    let families: BTreeSet<_> =
        HAIR.iter().filter(|(t, _)| tags.is_some_and(|tags| tags.contains(*t))).map(|(_, f)| *f).collect();
    (families.len() == 1).then(|| *families.first().expect("one hair family"))
}
fn split(members: Vec<usize>, tags: &Tags, ids: &[String]) -> Vec<Vec<usize>> {
    let families: Vec<_> = members.iter().map(|&i| hair(tags.get(&ids[i]))).collect();
    let mut counts = BTreeMap::<&str, usize>::new();
    for f in families.iter().flatten() {
        *counts.entry(f).or_default() += 1;
    }
    let mut big: Vec<_> = counts
        .iter()
        .filter(|(_, n)| **n >= 3 && **n as f64 >= 0.3 * members.len() as f64)
        .map(|(&f, &n)| (f, n))
        .collect();
    big.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    if big.len() < 2 {
        return vec![members];
    }
    let mut parts = vec![vec![]; big.len()];
    for (i, f) in members.into_iter().zip(families) {
        let k = big.iter().position(|(family, _)| Some(*family) == f).unwrap_or(0);
        parts[k].push(i);
    }
    parts
}
fn split_by_lead(members: Vec<usize>, ids: &[String], leads: &HashMap<String, String>) -> Vec<Vec<usize>> {
    let mut positions = HashMap::new();
    let mut parts: Vec<Vec<usize>> = Vec::new();
    // 按各部分首张图在原组里的顺序编号。
    for i in members {
        let lead = leads.get(&ids[i]);
        let pos = *positions.entry(lead).or_insert_with(|| {
            parts.push(Vec::new());
            parts.len() - 1
        });
        parts[pos].push(i);
    }
    parts
}
pub fn oc(snapshot: &Snapshot, f: &Matrix, entity: &str) -> Result<Value> {
    let c = snapshot.read()?;
    let pool = snapshot::strings(
        &c,
        &format!("SELECT m.uuid FROM memberships m WHERE m.entity=? AND {}", visible("m")),
        &[&entity],
    )?;
    let tags = tags(&c)?;
    let (ids, rows) = f.matrix(&pool);
    let groups: Vec<_> = numeric::groups(&rows, 0.06, 2, None)
        .map_err(Error::Invalid)?
        .into_iter()
        .flat_map(|g| split(g, &tags, &ids))
        .filter(|g| g.len() >= 2)
        .map(|g| g.into_iter().map(|i| ids[i].clone()).collect::<Vec<_>>())
        .collect();
    Ok(json!({"total":pool.len(),"groups":groups}))
}
pub fn cluster(snapshot: &Snapshot, f: &Matrix) -> Result<(usize, Value)> {
    let c = snapshot.read()?;
    let pool=snapshot::strings(&c,&format!("SELECT a.uuid FROM assets a WHERE {} AND NOT EXISTS(SELECT 1 FROM memberships m WHERE m.uuid=a.uuid) AND NOT EXISTS(SELECT 1 FROM deferred d WHERE d.uuid=a.uuid)",visible("a")),&[])?;
    let tags = tags(&c)?;
    let (ids, rows) = f.matrix(&pool);
    let groups: Vec<_> = numeric::groups(&rows, 0.07, 2, None)
        .map_err(Error::Invalid)?
        .into_iter()
        .flat_map(|g| split(g, &tags, &ids))
        .collect();
    let page = json!(groups.iter().flatten().map(|&i| &ids[i]).collect::<Vec<_>>()).to_string();
    let leads: HashMap<_, _> = first_leads(&c, &page)?.into_iter().collect();
    let groups: Vec<_> =
        groups.into_iter().flat_map(|g| split_by_lead(g, &ids, &leads)).filter(|g| g.len() >= 2).collect();
    let n = groups.len();
    let traits = traits();
    let centres =
        groups.iter().map(|g| numeric::centre(&rows, g).map_err(Error::Invalid)).collect::<Result<Vec<_>>>()?;
    let distances = numeric::distances(&centres, &centres).map_err(Error::Invalid)?;
    let share: Vec<Vec<f64>> = groups
        .iter()
        .map(|g| {
            traits
                .iter()
                .map(|t| {
                    g.iter().filter(|&&i| tags.get(&ids[i]).is_some_and(|tags| tags.contains(*t))).count() as f64
                        / g.len() as f64
                })
                .collect()
        })
        .collect();
    let (weight, profile) = trait_profiles(&share, traits.len());
    #[cfg(target_os = "windows")]
    let alike = trait_alike(&profile).map_err(Error::Invalid)?;
    let families: BTreeSet<_> = HAIR.iter().map(|(_, f)| *f).collect();
    let dominant: Vec<_> = share
        .iter()
        .map(|s| {
            let mut best = (None, 0.0);
            for &f in &families {
                let total =
                    HAIR.iter().enumerate().filter(|(_, (_, family))| *family == f).map(|(t, _)| s[t]).sum::<f64>();
                if best.0.is_none() || total > best.1 {
                    best = (Some(f), total);
                }
            }
            if best.1 >= 0.6 {
                best.0
            } else {
                None
            }
        })
        .collect();
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut clusters = vec![];
    let mut info = vec![];
    for k in 0..n {
        let mut distances = distances[k].clone();
        distances[k] = 9.0;
        let mut near = (Value::Null, Value::Null, Value::Null);
        for j in numeric::argsort(&distances) {
            if distances[j] > 0.12 {
                break;
            }
            if dominant[k].is_some() && dominant[j].is_some() && dominant[k] != dominant[j] {
                continue;
            }
            if distances[j] <= 0.08 {
                near = (json!(j + 1), json!(distances[j] as f64), json!("look"));
                break;
            }
            #[cfg(target_os = "windows")]
            let alike = alike[k][j];
            #[cfg(not(target_os = "windows"))]
            let alike = profile[k].iter().zip(&profile[j]).map(|(a, b)| a * b).sum::<f64>();
            if alike >= 0.85 {
                near = (json!(j + 1), json!(distances[j] as f64), json!("trait"));
                break;
            }
        }
        let score: Vec<_> = share[k].iter().zip(&weight).map(|(s, w)| -s * w).collect();
        let described: Vec<_> = numeric::argsort(&score)
            .into_iter()
            .filter(|&t| share[k][t] >= 0.5)
            .take(4)
            .map(|t| traits[t].replace('_', " "))
            .collect();
        // 这个字段一直是七列缓存行里的一个 JSON 字符串。
        let described = format!(
            "[{}]",
            described.iter().map(|s| serde_json::to_string(s).expect("trait string")).collect::<Vec<_>>().join(", ")
        );
        info.push(json!([k + 1, groups[k].len(), near.0, near.1, stamp, near.2, described]));
        clusters.extend(groups[k].iter().map(|&i| json!([ids[i], k + 1])));
    }
    Ok((n, json!({"clusters":clusters,"cluster_info":info})))
}

fn trait_profiles(share: &[Vec<f64>], width: usize) -> (Vec<f64>, Vec<Vec<f64>>) {
    let n = share.len();
    let weight: Vec<_> =
        (0..width).map(|t| ((n + 1) as f64 / (share.iter().filter(|s| s[t] >= 0.5).count() + 1) as f64).ln()).collect();
    let profile: Vec<Vec<f64>> = share
        .iter()
        .map(|s| {
            let mut p: Vec<_> = s.iter().zip(&weight).map(|(s, w)| s * w).collect();
            #[cfg(target_os = "windows")]
            let norm = numeric::sum64(&p.iter().map(|v| v * v).collect::<Vec<_>>()).sqrt().max(1e-9);
            #[cfg(not(target_os = "windows"))]
            let norm = p.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-9);
            for x in &mut p {
                *x /= norm;
            }
            p
        })
        .collect();
    (weight, profile)
}

// NumPy 对同一缓冲区计算整个 profile 乘积，选用的是 DSYRK。
// 逐对独立做标量点积会在 trait 阈值处改变累加结果的位。
#[cfg(target_os = "windows")]
fn trait_alike(profile: &[Vec<f64>]) -> std::result::Result<Vec<Vec<f64>>, String> {
    if profile.is_empty() {
        return Ok(vec![]);
    }
    let width = profile[0].len();
    let values: Vec<_> = profile.iter().flatten().copied().collect();
    let product =
        illustash_platform::platform_research_math::cross64(&values, profile.len(), &values, profile.len(), width)?;
    Ok(product.chunks(profile.len()).map(|row| row.to_vec()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires explicit independently generated NumPy trait reference"]
    fn fixed_windows_trait_profile_norm_and_alike_reference() {
        // 这项显式的开发检查只使用对固定合成 share 独立算出的 NumPy 参考字节，从不使用候选的输出。
        let path = std::env::var_os("ILLUSTASH_RESEARCH_TRAIT_REFERENCE")
            .expect("explicit independent NumPy trait reference required");
        let reference: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for case in reference.as_array().unwrap() {
            let share: Vec<Vec<f64>> = serde_json::from_value(case["share"].clone()).unwrap();
            let (weights, profile) = trait_profiles(&share, share[0].len());
            let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
            assert_eq!(json!(bits(&weights)), case["weight_bits"], "weight");
            assert_eq!(
                json!(profile.iter().map(|p| bits(p)).collect::<Vec<_>>()),
                case["profile_bits"],
                "profile/norm"
            );
            let alike: Vec<Vec<_>> = trait_alike(&profile).unwrap().iter().map(|row| bits(row)).collect();
            assert_eq!(json!(alike), case["alike_bits"], "float64 final trait product");
        }
    }
    #[test]
    fn hair_split_keeps_uncoloured_members_in_largest_part() {
        let ids: Vec<_> = (0..8).map(|i| i.to_string()).collect();
        let mut tags = Tags::new();
        for id in ids.iter().take(3) {
            tags.insert(id.clone(), BTreeSet::from(["blonde_hair".into()]));
        }
        for id in ids.iter().take(7).skip(3) {
            tags.insert(id.clone(), BTreeSet::from(["black_hair".into()]));
        }
        let groups = split((0..8).collect(), &tags, &ids);
        assert_eq!(groups, vec![vec![3, 4, 5, 6, 7], vec![0, 1, 2]]);
    }
}
