//! 自建标签本机学习的样本选择、组留出和候选排序。
use crate::example_scores::{self, Examples, Matrix, Row, Scorer};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

pub(crate) const MIN_POSITIVES: usize = 5;
const BACKGROUND: usize = 2000;
const MAX_CANDIDATES: usize = 200;
const PERCENTILE: f64 = 0.20;
const FOLDS: usize = 5;
// 私有离线评估固定 C=0.3，和共享线性头的 l2 互为倒数。
const SCORER: Scorer = Scorer::Linear { l2: 1.0 / 0.3 };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Yes,
    No,
    Clear,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Asset {
    pub(crate) uuid: String,
    pub(crate) group: Option<String>,
    pub(crate) visible: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Label {
    pub(crate) verdicts: BTreeMap<String, Verdict>,
}

#[derive(Debug)]
pub(crate) struct Plan {
    pub(crate) inputs: String,
    pub(crate) positives: usize,
    pub(crate) negatives: usize,
    pub(crate) cutoff: Option<f32>,
    pub(crate) candidates: Vec<(String, f32)>,
}

pub(crate) fn make_plan(matrix: &Matrix, key: &str, label: &Label, assets: &[Asset], feature_version: &str) -> Plan {
    let mut positives = BTreeMap::new();
    let mut negatives = BTreeMap::new();
    let mut available = Vec::new();
    for asset in assets {
        let Some(row) = matrix.row(&asset.uuid) else {
            continue;
        };
        let group = (asset.group.is_some(), asset.group.as_deref().unwrap_or(&asset.uuid));
        match label.verdicts.get(&asset.uuid) {
            Some(Verdict::Yes) => {
                positives.entry(group).or_insert(row);
            }
            Some(Verdict::No) => {
                negatives.entry(group).or_insert(row);
            }
            None if asset.visible => available.push((asset.uuid.as_str(), row)),
            _ => {}
        }
    }
    // 同一重复组已有互相冲突的人工裁决时，不给线性头一对矛盾样本。
    let conflicts: HashSet<_> = positives.keys().filter(|key| negatives.contains_key(*key)).copied().collect();
    positives.retain(|key, _| !conflicts.contains(key));
    negatives.retain(|key, _| !conflicts.contains(key));
    let mut positives: Vec<_> = positives.into_values().collect();
    let mut negatives: Vec<_> = negatives.into_values().collect();
    positives.sort_unstable();
    negatives.sort_unstable();

    let mut background: Vec<_> =
        available.iter().map(|&(uuid, row)| (sha256_fields([key.as_bytes(), uuid.as_bytes()]), row)).collect();
    if background.len() > BACKGROUND {
        background.select_nth_unstable(BACKGROUND);
        background.truncate(BACKGROUND);
    }
    let mut background: Vec<_> = background.into_iter().map(|(_, row)| row).collect();
    background.sort_unstable();
    let mut digest = Sha256::new();
    hash_field(&mut digest, feature_version.as_bytes());
    for group in [&positives, &negatives, &background] {
        hash_field(&mut digest, &(group.len() as u64).to_le_bytes());
        for &row in group {
            hash_field(&mut digest, matrix.uuid(row).as_bytes());
        }
    }
    let inputs: String = format!("{:x}", digest.finalize())[..16].into();

    let examples = Examples { positives: &positives, negatives: &negatives, background: &background, exclude: &[] };
    let threshold = cutoff(matrix, &examples);
    let results = threshold.map_or_else(Vec::new, |threshold| {
        let candidates: Vec<_> =
            available.iter().map(|&(_, row)| row).filter(|row| background.binary_search(row).is_err()).collect();
        let cap = MAX_CANDIDATES.min(10 * positives.len());
        example_scores::top(matrix, &examples, &SCORER, &candidates, cap)
            .into_iter()
            .take_while(|&(_, score)| score >= threshold)
            .map(|(row, score)| (matrix.uuid(row).to_owned(), score))
            .collect()
    });
    Plan { inputs, positives: positives.len(), negatives: negatives.len(), cutoff: threshold, candidates: results }
}

fn cutoff(matrix: &Matrix, examples: &Examples<'_>) -> Option<f32> {
    if examples.positives.len() < MIN_POSITIVES {
        return None;
    }
    let mut held_out = Vec::with_capacity(examples.positives.len());
    for fold in 0..FOLDS {
        let (held, train): (Vec<_>, Vec<_>) =
            examples.positives.iter().copied().enumerate().partition(|(i, _)| i % FOLDS == fold);
        let train: Vec<Row> = train.into_iter().map(|(_, row)| row).collect();
        let held: Vec<Row> = held.into_iter().map(|(_, row)| row).collect();
        let fold_examples = Examples { positives: &train, ..*examples };
        held_out.extend(
            example_scores::top(matrix, &fold_examples, &SCORER, &held, held.len()).into_iter().map(|(_, score)| score),
        );
    }
    if held_out.len() != examples.positives.len() {
        return None;
    }
    held_out.sort_unstable_by(f32::total_cmp);
    let index = (held_out.len() - 1) as f64 * PERCENTILE;
    let lower = index.floor() as usize;
    let upper = index.ceil() as usize;
    let weight = index.fract() as f32;
    Some(held_out[lower] * (1.0 - weight) + held_out[upper] * weight)
}

pub(crate) fn hash_field(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

fn sha256_fields<const N: usize>(fields: [&[u8]; N]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for field in fields {
        hash_field(&mut digest, field);
    }
    digest.finalize().into()
}

#[cfg(test)]
mod tests;
