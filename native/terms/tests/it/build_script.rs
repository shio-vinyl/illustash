//! native/terms/build.rs 的 Git 监视路径与构建号解析测试（#327、#460）。
#![allow(clippy::unwrap_used, reason = "集成测试里 unwrap 就是断言")]

#[path = "../../build.rs"]
#[allow(dead_code)]
mod script;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// 临时测试目录；离开作用域（含断言失败）时清理。
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("illustash-build-script-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    // 给 Git 的目录保留系统临时路径。只在比较监视路径时规范化，避免给 Git for Windows 传入设备路径。
    Scratch(dir)
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(cwd).output().unwrap();
    assert!(out.status.success(), "git {args:?} failed in {}: {}", cwd.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    fs::write(dir.join("source-revision.txt"), "$Format:%h$\n").unwrap();
    git(dir, &["add", "source-revision.txt"]);
    git(dir, &["commit", "-m", "init"]);
}

fn canonical_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.into_iter().map(|p| fs::canonicalize(p).unwrap()).collect()
}

#[test]
fn packed_branch_watches_branch_reflog_instead_of_shared_refs_heads() {
    let _env = crate::shared();
    let root = scratch("packed-reflog");
    let repo = root.join("repo");
    init_repo(&repo);
    git(&repo, &["branch", "curr"]);
    git(&repo, &["branch", "topic/nested"]);
    git(&repo, &["branch", "other"]);

    let wt_curr = root.join("wt-curr");
    let wt_nested = root.join("wt-nested");
    let wt_other = root.join("wt-other");
    git(&repo, &["worktree", "add", wt_curr.to_str().unwrap(), "curr"]);
    git(&repo, &["worktree", "add", wt_nested.to_str().unwrap(), "topic/nested"]);
    git(&repo, &["worktree", "add", wt_other.to_str().unwrap(), "other"]);
    git(&repo, &["pack-refs", "--all"]);

    let refs_heads = fs::canonicalize(repo.join(".git/refs/heads")).unwrap();
    let curr_head = fs::canonicalize(repo.join(".git/worktrees/wt-curr/HEAD")).unwrap();
    let curr_log = fs::canonicalize(repo.join(".git/logs/refs/heads/curr")).unwrap();
    let nested_head = fs::canonicalize(repo.join(".git/worktrees/wt-nested/HEAD")).unwrap();
    let nested_log = fs::canonicalize(repo.join(".git/logs/refs/heads/topic/nested")).unwrap();

    // 打包后当前分支独立 ref 文件不存在，应监视 HEAD 与同分支 reflog，而非共享的 refs/heads 目录。
    let watched_curr = canonical_paths(script::git_watch_paths(&wt_curr));
    assert_eq!(watched_curr, vec![curr_head.clone(), curr_log.clone()]);
    assert!(!watched_curr.contains(&refs_heads));

    let watched_nested = canonical_paths(script::git_watch_paths(&wt_nested));
    assert_eq!(watched_nested, vec![nested_head, nested_log]);
    assert!(!watched_nested.contains(&refs_heads));

    let before_build = script::resolve_build_id(&wt_curr, &wt_curr.join("source-revision.txt"));
    assert_eq!(before_build, git(&wt_curr, &["rev-parse", "--short", "HEAD"]));
    let log_before = fs::read(&curr_log).unwrap();

    // 其他工作树提交并再次打包 ref，不应改动当前分支被监视的 reflog 或构建号。
    git(&wt_other, &["commit", "--allow-empty", "-m", "other commit"]);
    git(&repo, &["pack-refs", "--all"]);
    assert_eq!(fs::read(&curr_log).unwrap(), log_before);
    assert_eq!(script::resolve_build_id(&wt_curr, &wt_curr.join("source-revision.txt")), before_build);

    // 当前分支提交后，reflog 被追加（触发重跑）、构建号更新，且监视切换回新生成的独立 ref 文件。
    git(&wt_curr, &["commit", "--allow-empty", "-m", "curr commit"]);
    let log_after = fs::read(&curr_log).unwrap();
    assert_ne!(log_after, log_before);
    let after_build = script::resolve_build_id(&wt_curr, &wt_curr.join("source-revision.txt"));
    assert_eq!(after_build, git(&wt_curr, &["rev-parse", "--short", "HEAD"]));
    assert_ne!(after_build, before_build);

    let curr_loose = fs::canonicalize(repo.join(".git/refs/heads/curr")).unwrap();
    assert_eq!(canonical_paths(script::git_watch_paths(&wt_curr)), vec![curr_head, curr_loose]);
}

#[test]
fn packed_branch_without_reflog_falls_back_to_existing_parent_dir() {
    let _env = crate::shared();
    let root = scratch("packed-no-reflog");
    let repo = root.join("repo");
    init_repo(&repo);
    git(&repo, &["branch", "curr"]);

    let wt_curr = root.join("wt-curr");
    git(&repo, &["worktree", "add", wt_curr.to_str().unwrap(), "curr"]);
    git(&repo, &["pack-refs", "--all"]);
    fs::remove_file(repo.join(".git/logs/refs/heads/curr")).unwrap();

    let curr_head = fs::canonicalize(repo.join(".git/worktrees/wt-curr/HEAD")).unwrap();
    let refs_heads = fs::canonicalize(repo.join(".git/refs/heads")).unwrap();
    assert_eq!(canonical_paths(script::git_watch_paths(&wt_curr)), vec![curr_head, refs_heads]);
}

#[test]
fn detached_head_watches_only_head_and_tracks_updates() {
    let _env = crate::shared();
    let root = scratch("detached-head");
    let repo = root.join("repo");
    init_repo(&repo);

    let wt_detached = root.join("wt-detached");
    git(&repo, &["worktree", "add", "--detach", wt_detached.to_str().unwrap(), "HEAD"]);

    let head = fs::canonicalize(repo.join(".git/worktrees/wt-detached/HEAD")).unwrap();
    assert_eq!(canonical_paths(script::git_watch_paths(&wt_detached)), vec![head]);

    let first = script::resolve_build_id(&wt_detached, &wt_detached.join("source-revision.txt"));
    assert_eq!(first, git(&wt_detached, &["rev-parse", "--short", "HEAD"]));

    git(&wt_detached, &["commit", "--allow-empty", "-m", "detached commit"]);
    let second = script::resolve_build_id(&wt_detached, &wt_detached.join("source-revision.txt"));
    assert_eq!(second, git(&wt_detached, &["rev-parse", "--short", "HEAD"]));
    assert_ne!(second, first);
}

#[test]
fn source_archive_reads_exported_revision_without_git_watches() {
    let _env = crate::shared();
    let root = scratch("source-archive");
    let revision = root.join("source-revision.txt");

    // 源码归档没有 .git，应返回空监视列表并读取 export-subst 写入的提交短哈希。
    fs::write(&revision, "a1b2c3d4\n").unwrap();
    assert!(script::git_watch_paths(&root).is_empty());
    assert_eq!(script::resolve_build_id(&root, &revision), "a1b2c3d4");

    // 未替换的占位符不是十六进制提交哈希，应被忽略。
    fs::write(&revision, "$Format:%h$\n").unwrap();
    assert_eq!(script::resolve_build_id(&root, &revision), "");
}
