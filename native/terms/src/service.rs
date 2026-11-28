//! 父进程私有的逐行 JSON 通道；无端口、无共享身份、无自动重放。
//! 命令顺序执行，事务与 CLI 共用。stdin 关闭即退出；崩溃由宿主在下次请求时重启。
use crate::{cli, strict_json, Error, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;

fn request(line: &str, historical_index: bool) -> Result<Value> {
    let args = strict_json::parse(line).map_err(Error::Invalid)?;
    let db = args.get("db").and_then(Value::as_str).ok_or_else(|| Error::Invalid("请求缺少数据库路径".into()))?;
    let input = args.get("input").ok_or_else(|| Error::Invalid("请求缺少命令".into()))?;
    crate::app_service::character_request_with(Path::new(db), input, historical_index)
}

pub(crate) fn run(historical_index: bool) -> Result<Value> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let response = match request(&line?, historical_index) {
            Ok(result) => json!({"code": 0, "result": result}),
            Err(error) => {
                let (code, message) = cli::error_response(error);
                json!({"code": code, "error": message})
            }
        };
        writeln!(out, "{response}")?;
        out.flush()?;
    }
    Ok(json!({"closed": true}))
}
