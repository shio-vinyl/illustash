//! 会话内的例图特征快照。切换图库释放整个缓存；长期不用时按空间释放矩阵。
use crate::{
    example_disk,
    example_features::{self, Space},
    example_scores::Matrix,
    Result,
};
use rusqlite::Connection;
use std::{
    fs,
    io::ErrorKind,
    path::Path,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant, SystemTime},
};

#[derive(Default, Debug)]
pub(crate) struct Cache(Arc<Inner>);

#[derive(Default, Debug)]
struct Inner {
    state: Mutex<State>,
    wake: Condvar,
}

pub(crate) type BaseStamp = [Option<(u64, SystemTime)>; 2];

#[derive(Default)]
struct State {
    source: Option<Connection>,
    tags: Option<Cached>,
    ccip: Option<Cached>,
    combined: Option<Cached>,
    reaping: bool,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureCache").finish_non_exhaustive()
    }
}

struct Cached {
    data_version: i64,
    files: BaseStamp,
    matrix: Arc<Matrix>,
    expires: Instant,
}

const IDLE: Duration = Duration::from_secs(600);

impl Cache {
    pub(crate) fn get(&self, source: &Path, base: &Path, space: Space) -> Result<Arc<Matrix>> {
        let mut state = self.0.state.lock().expect("example cache lock poisoned");
        let State { source: reader, tags, ccip, combined, .. } = &mut *state;
        if reader.is_none() {
            *reader = Some(crate::app_library::read_connection(source)?);
        }
        let c = reader.as_mut().expect("source reader was opened");
        // 保持同一个只读连接。其他连接的提交（含原地 UPDATE）改变 data_version。
        let data_version = |c: &Connection| c.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0));
        let version = data_version(c)?;
        let files = base_stamp(base, space)?;
        let slot = match space {
            Space::Tags => tags,
            Space::Ccip => ccip,
            Space::TagsCcip => combined,
        };
        if let Some(cached) = slot
            .as_mut()
            .filter(|cached| cached.data_version == version && cached.files == files && cached.expires > Instant::now())
        {
            cached.expires = Instant::now() + IDLE;
            return Ok(cached.matrix.clone());
        }
        *slot = None;
        let disk = if space == Space::Tags {
            example_disk::path(base).zip(example_disk::source_stamp(source).ok())
        } else {
            None
        };
        let stored = disk.as_ref().and_then(|(path, stamp)| example_disk::read(path, *stamp));
        let stamp_still_matches = |stamp| example_disk::source_stamp(source).is_ok_and(|now| now == stamp);
        let (matrix, rebuild) = match stored {
            Some(matrix)
                if data_version(c)? == version
                    && disk.as_ref().is_some_and(|(_, stamp)| stamp_still_matches(*stamp)) =>
            {
                (Arc::new(matrix), false)
            }
            _ => {
                let tx = c.transaction()?;
                let matrix = Arc::new(example_features::load(&tx, base, space)?);
                tx.commit()?;
                (matrix, true)
            }
        };
        // 源库更新可能在取版本之后、固定快照之前发生，这时本次仍可用读到的事务快照，
        // 但不把它标作已知版本缓存。基础文件在途变化时连本次结果也无法确认。
        if base_stamp(base, space)? != files {
            return Err(crate::Error::Conflict("角色特征正在更新，请重试".into()));
        }
        if data_version(c)? != version {
            return Ok(matrix);
        }
        if let Some((path, stamp)) =
            disk.filter(|(_, stamp)| rebuild && !matrix.is_empty() && stamp_still_matches(*stamp))
        {
            // 这是可删除的派生文件。写失败时仍可用本次读到的源库快照。
            if let Err(error) = example_disk::write(&path, stamp, &matrix) {
                eprintln!("未能写入 WD 例图缓存：{error}");
            }
        }
        *slot = Some(Cached { data_version: version, files, matrix: matrix.clone(), expires: Instant::now() + IDLE });
        if !state.reaping {
            let inner = self.0.clone();
            std::thread::Builder::new().name("illustash-example-cache".into()).spawn(move || reap(&inner))?;
            state.reaping = true;
        }
        self.0.wake.notify_one();
        Ok(matrix)
    }

    pub(crate) fn clear(&self) {
        let mut state = self.0.state.lock().expect("example cache lock poisoned");
        state.source = None;
        state.tags = None;
        state.ccip = None;
        state.combined = None;
        self.0.wake.notify_one();
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        self.clear();
    }
}

pub(crate) fn base_stamp(base: &Path, space: Space) -> Result<BaseStamp> {
    let mut files = [None; 2];
    if space != Space::Tags {
        for (slot, name) in files.iter_mut().zip(["full-manifest.csv", "full-features.npy"]) {
            match fs::metadata(base.join(name)) {
                Ok(meta) => *slot = Some((meta.len(), meta.modified()?)),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(files)
}

fn reap(inner: &Inner) {
    let mut state = inner.state.lock().expect("example cache lock poisoned");
    loop {
        let now = Instant::now();
        let State { tags, ccip, combined, .. } = &mut *state;
        let mut next = None;
        for slot in [tags, ccip, combined] {
            if let Some(cached) = slot {
                if cached.expires <= now {
                    *slot = None;
                } else {
                    next = Some(next.map_or(cached.expires, |n: Instant| n.min(cached.expires)));
                }
            }
        }
        let Some(next) = next else {
            state.source = None;
            state.reaping = false;
            return;
        };
        state =
            inner.wake.wait_timeout(state, next.saturating_duration_since(now)).expect("example cache lock poisoned").0;
    }
}
