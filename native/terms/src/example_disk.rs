//! 可重建的 WD 稀疏特征文件。只在内存缺页时访问，源库及其 WAL 都参与失效。
use crate::example_scores::{Matrix, Tags, Vectors};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

const FORMAT: &[u8] = b"illustash-wd-idf-1-scope\n";
pub(crate) type Stamp = [u8; 32];

/// SQLite 的物理版本标记和文件状态。普通 SQLite 提交会改主文件或 WAL，检查点也会使缓存失效。
pub(crate) fn source_stamp(source: &Path) -> io::Result<Stamp> {
    let mut wal = source.as_os_str().to_owned();
    wal.push("-wal");
    let mut hash = Sha256::new();
    for (path, optional) in [(source, false), (Path::new(&wal), true)] {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(e) if optional && e.kind() == ErrorKind::NotFound => {
                hash.update([0]);
                continue;
            }
            Err(e) => return Err(e),
        };
        let meta = file.metadata()?;
        let ns = |t: std::time::SystemTime| t.duration_since(UNIX_EPOCH).map_or(0, |n| n.as_nanos());
        hash.update([1]);
        hash.update(meta.len().to_le_bytes());
        hash.update(ns(meta.modified()?).to_le_bytes());
        hash.update(meta.created().ok().map_or(0, ns).to_le_bytes());
        // SQLite 头部的 change counter、WAL 的 salt 提供与文件时间独立的证据。
        let mut header = Vec::with_capacity(100);
        Read::by_ref(&mut file).take(100).read_to_end(&mut header)?;
        hash.update(header.len().to_le_bytes());
        hash.update(header);
    }
    Ok(hash.finalize().into())
}

pub(crate) fn path(base: &Path) -> Option<PathBuf> {
    Some(base.parent()?.join("cache").join("wd-tags-idf-v1.bin"))
}

pub(crate) fn read(path: &Path, stamp: Stamp) -> Option<Matrix> {
    let mut f = File::open(path).ok()?;
    let mut magic = [0; FORMAT.len()];
    let mut key = [0; 32];
    let mut checksum = [0; 32];
    f.read_exact(&mut magic).ok()?;
    f.read_exact(&mut key).ok()?;
    if magic != FORMAT || key != stamp {
        return None;
    }
    f.read_exact(&mut checksum).ok()?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).ok()?;
    if <Stamp>::from(Sha256::digest(&bytes)) != checksum {
        return None;
    }
    decode(&bytes)
}

fn decode(mut data: &[u8]) -> Option<Matrix> {
    let take_u32 = |data: &mut &[u8]| {
        let (head, rest) = data.split_first_chunk()?;
        *data = rest;
        Some(u32::from_le_bytes(*head))
    };
    let width = take_u32(&mut data)? as usize;
    let count = take_u32(&mut data)? as usize;
    let mut uuids = Vec::<String>::new();
    let mut offsets = Vec::new();
    let mut entries = Vec::new();
    for _ in 0..count {
        let len = take_u32(&mut data)? as usize;
        let uuid = std::str::from_utf8(data.get(..len)?).ok()?;
        if uuid.is_empty() || uuids.last().is_some_and(|id| id.as_str() >= uuid) {
            return None;
        }
        data = &data[len..];
        let size = take_u32(&mut data)? as usize;
        if size == 0 || size > width {
            return None;
        }
        uuids.push(uuid.to_owned());
        offsets.push(entries.len());
        for _ in 0..size {
            let column = take_u32(&mut data)?;
            let value = f32::from_bits(take_u32(&mut data)?);
            if column as usize >= width || !value.is_finite() || value <= 0.0 {
                return None;
            }
            entries.push((column, value));
        }
    }
    offsets.push(entries.len());
    if !data.is_empty() {
        return None;
    }
    Some(Matrix { uuids, vectors: Vectors::Tags(Tags { width, offsets, entries }) })
}

pub(crate) fn write(path: &Path, stamp: Stamp, matrix: &Matrix) -> io::Result<()> {
    let Vectors::Tags(tags) = &matrix.vectors else {
        return Ok(());
    };
    let u32_bytes =
        |value: usize| u32::try_from(value).map(u32::to_le_bytes).map_err(|_| io::Error::other("WD 特征缓存字段过大"));
    let mut bytes = Vec::with_capacity(tags.entries.len().saturating_mul(8));
    bytes.extend(u32_bytes(tags.width)?);
    bytes.extend(u32_bytes(matrix.len())?);
    for (id, bounds) in matrix.uuids.iter().zip(tags.offsets.windows(2)) {
        bytes.extend(u32_bytes(id.len())?);
        bytes.extend(id.bytes());
        bytes.extend(u32_bytes(bounds[1] - bounds[0])?);
        for &(column, value) in &tags.entries[bounds[0]..bounds[1]] {
            bytes.extend(column.to_le_bytes());
            bytes.extend(value.to_le_bytes());
        }
    }
    let Some(parent) = path.parent() else {
        return Err(io::Error::other("WD 特征缓存没有父目录"));
    };
    fs::create_dir_all(parent)?;
    let mut nonce = [0; 8];
    getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
    let temp = path.with_extension(format!("{:016x}.tmp", u64::from_ne_bytes(nonce)));
    let result = (|| {
        let mut file = File::options().write(true).create_new(true).open(&temp)?;
        file.write_all(FORMAT)?;
        file.write_all(&stamp)?;
        file.write_all(&Sha256::digest(&bytes))?;
        file.write_all(&bytes)?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
