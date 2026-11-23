//! 豆包工作适配器真机调试工具（含调度器全链路）。
//!
//! 用法：
//! ```bash
//! # 只读：只查额度与权威签到状态
//! cargo run --example doubao_probe -- --cookie-file /tmp/db_cookie.txt --read-only
//! # 完整：走 DoubaoWorkAdapter 的真实领取
//! cargo run --example doubao_probe -- --cookie-file /tmp/db_cookie.txt
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use qwitch_lib::adapter::doubaowork::DoubaoWorkAdapter;
use qwitch_lib::adapter::ToolAdapter;
use qwitch_lib::domain::CredentialEnvelope;
use qwitch_lib::platform::{HostError, KeyExpectation, PlatformHost};

struct ProbeHost {
    profile_root: std::path::PathBuf,
}

fn unsupported<T>(what: &str) -> Result<T, HostError> {
    Err(HostError::OperationFailed(format!(
        "probe host: {what} 未实现"
    )))
}

#[async_trait]
impl PlatformHost for ProbeHost {
    fn qwitch_data_dir(&self) -> Result<std::path::PathBuf, HostError> {
        Ok(std::env::temp_dir().join("qwitch_probe"))
    }

    fn client_config_dir(&self, tool_id: &str) -> Result<std::path::PathBuf, HostError> {
        if tool_id == "doubaowork" {
            Ok(self.profile_root.clone())
        } else {
            unsupported("client_config_dir")
        }
    }

    fn detect_client_executable(&self, _tool_id: &str) -> Option<std::path::PathBuf> {
        None
    }

    fn is_client_running(&self, _tool_id: &str) -> Result<bool, HostError> {
        Ok(false)
    }

    async fn terminate_client_gracefully(
        &self,
        _tool_id: &str,
        _timeout_ms: u64,
    ) -> Result<(), HostError> {
        unsupported("terminate_client_gracefully")
    }

    fn launch_client(
        &self,
        _executable: &std::path::Path,
        _args: &[String],
    ) -> Result<(), HostError> {
        unsupported("launch_client")
    }

    fn get_or_create_master_key(
        &self,
        _expectation: &KeyExpectation,
    ) -> Result<[u8; 32], HostError> {
        unsupported("get_or_create_master_key")
    }

    fn set_launch_at_login(&self, _enabled: bool) -> Result<(), String> {
        Err("probe host: set_launch_at_login 未实现".to_string())
    }
}

fn parse_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn print_quota(tag: &str, q: &qwitch_lib::domain::QuotaSnapshot) {
    println!(
        "[{tag}] plan={:?} claim_status={:?}",
        q.plan_name, q.claim_status
    );
    for b in &q.buckets {
        println!(
            "      {:<28} {}/{} {}  reset_at={:?}",
            b.label, b.remaining, b.total, b.unit, b.reset_at
        );
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let read_only = args.iter().any(|a| a == "--read-only");
    let rounds: usize = parse_arg(&args, "--rounds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let cookie = match parse_arg(&args, "--cookie-file") {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("读取 Cookie 文件失败 {path}: {e}");
                std::process::exit(2);
            }
        },
        None => match parse_arg(&args, "--cookie") {
            Some(c) => c,
            None => {
                eprintln!("用法: doubao_probe --cookie-file <PATH> [--read-only] [--rounds N]");
                std::process::exit(2);
            }
        },
    };

    let home = std::env::var("HOME").unwrap_or_default();
    let profile_root =
        std::path::PathBuf::from(format!("{home}/Library/Application Support/DoubaoWork"));
    if let Ok(bytes) = std::fs::read(profile_root.join("Local State")) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            println!(
                "device_id={:?} install_id={:?}",
                v.pointer("/aha/device/device_id").and_then(|x| x.as_str()),
                v.pointer("/aha/device/install_id").and_then(|x| x.as_str())
            );
        }
    }

    let host: Arc<dyn PlatformHost> = Arc::new(ProbeHost { profile_root });
    let adapter = DoubaoWorkAdapter::with_host(Arc::clone(&host));

    println!("\n=== 1) 校验 Cookie ===");
    let cred: CredentialEnvelope = match adapter.parse_manual_token(&cookie).await {
        Ok(c) => {
            println!("uid={} name={:?}", c.uid, c.display_name);
            c
        }
        Err(e) => {
            eprintln!("Cookie 校验失败: {e}");
            std::process::exit(1);
        }
    };

    println!("\n=== 2) 初始状态 ===");
    let mut quota = match adapter.inspect_quota(&cred).await {
        Ok(q) => {
            print_quota("before", &q);
            Some(q)
        }
        Err(e) => {
            eprintln!("查询额度失败: {e}");
            None
        }
    };

    if read_only {
        println!("\n[--read-only] 结束。");
        return;
    }

    for r in 1..=rounds {
        println!("\n=== 3.{r}) 第 {r} 轮 claim_daily ===");
        match adapter.claim_daily(&cred, quota.as_ref()).await {
            Ok(res) => {
                println!("claimed        = {}", res.claimed);
                println!("reward_summary = {}", res.reward_summary);
                if let Some(q) = &res.updated_quota {
                    print_quota("after", q);
                } else {
                    println!("updated_quota  = None");
                }
            }
            Err(e) => println!("Err = {e}"),
        }
        // 每轮之间等一下，给平台异步确认留时间
        if r < rounds {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        quota = adapter.inspect_quota(&cred).await.ok();
        if let Some(q) = &quota {
            print_quota("recheck", q);
        }
    }
}
