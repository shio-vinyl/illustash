//! 从源库读取例图特征。宿主缓存和离线评估使用同一读取入口。
use crate::{
    example_scores::{Ccip, Matrix, Tags, Vectors},
    Result,
};
use half::slice::HalfFloatSliceExt;
use rusqlite::Connection;
use std::{collections::HashMap, path::Path, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Tags,
    Ccip,
    TagsCcip,
}

impl Space {
    /// 表示算法和识别模型一起组成特征版本，模型版本由调用方从实际输入取得。
    pub fn version(self, wd: &str, ccip: &str) -> String {
        match self {
            Self::Tags => format!("tags-idf/1@{wd}"),
            Self::Ccip => format!("ccip/1@{ccip}"),
            Self::TagsCcip => format!("tags+ccip/1@{wd}@{ccip}"),
        }
    }
}

pub fn load(source: &Connection, base: &Path, space: Space) -> Result<Matrix> {
    match space {
        Space::Tags => tags(source),
        Space::Ccip => ccip(source, base),
        Space::TagsCcip => {
            let tags = Arc::new(tags(source)?);
            let ccip = Arc::new(ccip(source, base)?);
            Ok(combine(tags, ccip))
        }
    }
}

fn tags(source: &Connection) -> Result<Matrix> {
    // 旧库可以缺少来源列。无法确认是 WD v3 的历史标签不进入这个特征空间。
    let has_source: bool =
        source.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('labels') WHERE name='source')", [], |r| {
            r.get(0)
        })?;
    if !has_source {
        let vectors = Vectors::Tags(Tags { width: 0, offsets: vec![0], entries: Vec::new() });
        return Ok(Matrix { uuids: Vec::new(), vectors });
    }
    // 原离线口径以有原图路径的在库资产为 IDF 的 N，包含没有 WD 标签的资产。
    const POOL: &str = "a.in_scope=1 AND coalesce(a.trashed,0)=0 AND coalesce(a.src_path,'')<>''";
    let count: usize = source.query_row(&format!("SELECT count(*) FROM assets a WHERE {POOL}"), [], |r| r.get(0))?;
    // 先按资产主键走，再用标签主键查每张图的 general 范围，不对整张概率表做临时排序。
    let mut stmt = source.prepare(&format!(
        "SELECT a.uuid,l.value,l.conf
         FROM assets a CROSS JOIN labels l ON l.uuid=a.uuid AND l.dim='general'
         WHERE {POOL} AND l.source='wd_v3' AND l.conf>=0.001
         ORDER BY a.uuid",
    ))?;
    let mut rows = stmt.query([])?;
    let mut uuids: Vec<String> = Vec::new();
    let mut columns: HashMap<String, u32> = HashMap::new();
    let mut frequency: Vec<usize> = Vec::new();
    let mut offsets = Vec::new();
    let mut entries: Vec<(u32, f32)> = Vec::new();
    while let Some(row) = rows.next()? {
        let uuid = row.get_ref(0)?.as_str().map_err(|err| row_error(0, err))?;
        if uuids.last().is_none_or(|u| u != uuid) {
            uuids.push(uuid.to_owned());
            offsets.push(entries.len());
        }
        let label = row.get_ref(1)?.as_str().map_err(|err| row_error(1, err))?;
        let probability: f64 = row.get(2)?;
        if !(0.001..=1.0).contains(&probability) {
            return Err(crate::Error::invalid("WD 标签概率越界"));
        }
        let column = if let Some(&column) = columns.get(label) {
            column
        } else {
            let column = u32::try_from(frequency.len()).map_err(|_| crate::Error::invalid("WD 标签种类过多"))?;
            columns.insert(label.to_owned(), column);
            frequency.push(0);
            column
        };
        frequency[column as usize] += usize::from(probability >= 0.35);
        entries.push((column, probability as f32));
    }
    offsets.push(entries.len());

    let n = count as f64 + 1.0;
    let idf: Vec<_> = frequency.into_iter().map(|df| ((n / (df as f64 + 1.0)).ln() + 1.0) as f32).collect();
    for bounds in offsets.windows(2) {
        let row = &mut entries[bounds[0]..bounds[1]];
        let mut squares = 0.0;
        for (column, weight) in &mut *row {
            *weight *= idf[*column as usize];
            squares += weight.powi(2);
        }
        let norm = squares.sqrt();
        for (_, weight) in row {
            *weight /= norm;
        }
    }
    let vectors = Vectors::Tags(Tags { width: idf.len(), offsets, entries });
    Ok(Matrix { uuids, vectors })
}

fn row_error(col: usize, err: rusqlite::types::FromSqlError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, Box::new(err))
}

fn ccip(source: &Connection, base: &Path) -> Result<Matrix> {
    let matrix = crate::research_compute::features::load_from(source, base)?;
    let mut ordered: Vec<_> = matrix.uuids.into_iter().zip(matrix.rows).collect();
    ordered.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut uuids = Vec::with_capacity(ordered.len());
    let mut values = vec![half::f16::ZERO; ordered.len() * matrix.dimension];
    for ((uuid, row), dest) in ordered.into_iter().zip(values.chunks_exact_mut(matrix.dimension)) {
        uuids.push(uuid);
        dest.convert_from_f32_slice(&row);
    }
    Ok(Matrix { uuids, vectors: Vectors::Ccip(Ccip { width: matrix.dimension, values }) })
}

/// 只有同时有两种特征的图片进入拼接空间。与单空间共享向量储存。
pub(crate) fn combine(tags: Arc<Matrix>, ccip: Arc<Matrix>) -> Matrix {
    let mut uuids = Vec::new();
    let mut rows = Vec::new();
    for (row, id) in tags.uuids.iter().enumerate() {
        if let Some(other) = ccip.row(id) {
            uuids.push(id.clone());
            rows.push((row, other));
        }
    }
    Matrix { uuids, vectors: Vectors::Combined { tags, ccip, rows } }
}
