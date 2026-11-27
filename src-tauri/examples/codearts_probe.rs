//! CodeArts 适配器真机调试：从本机 secretStorage 导入凭据，查询额度并真实领取每日签到。
//!
//! ```bash
//! cargo run --example codearts_probe
//! # STS 误标过期排查时可加 CODEARTS_PROBE_IGNORE_EXPIRY=1 让服务端判定
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use qwitch_lib::adapter::codearts::CodeArtsAdapter;
use qwitch_lib::adapter::ToolAdapter;
use qwitch_lib::platform::{HostError, KeyExpectation, PlatformHost};

struct ProbeHost;

#[async_trait]
impl PlatformHost for ProbeHost {
    fn qwitch_data_dir(&self) -> Result<std::path::PathBuf, HostError> {
        Ok(std::env::temp_dir().join("qwitch_probe"))
    }
    fn client_config_dir(&self, tool_id: &str) -> Result<std::path::PathBuf, HostError> {
        let home = std::env::var("HOME").unwrap_or_default();
        match tool_id {
            "codearts" => Ok(std::path::PathBuf::from(format!(
                "{home}/Library/Application Support/codearts"
            ))),
            _ => Err(HostError::OperationFailed("probe host".into())),
        }
    }
    fn detect_client_executable(&self, _t: &str) -> Option<std::path::PathBuf> {
        None
    }
    fn is_client_running(&self, _t: &str) -> Result<bool, HostError> {
        Ok(false)
    }
    async fn terminate_client_gracefully(&self, _t: &str, _ms: u64) -> Result<(), HostError> {
        Err(HostError::OperationFailed("probe host".into()))
    }
    fn launch_client(&self, _e: &std::path::Path, _a: &[String]) -> Result<(), HostError> {
        Err(HostError::OperationFailed("probe host".into()))
    }
    fn get_or_create_master_key(&self, _e: &KeyExpectation) -> Result<[u8; 32], HostError> {
        Err(HostError::OperationFailed("probe host".into()))
    }
    fn set_launch_at_login(&self, _e: bool) -> Result<(), String> {
        Err("probe host".into())
    }
}

#[tokio::main]
async fn main() {
    let host: Arc<dyn PlatformHost> = Arc::new(ProbeHost);
    let adapter = CodeArtsAdapter::with_host(Arc::clone(&host));

    println!("=== manifest ===");
    let m = adapter.manifest();
    println!("  id={} name={}", m.id, m.name);
    println!(
        "  quota_query={} daily_claim={}",
        m.capabilities.supports_quota_query, m.capabilities.supports_daily_claim
    );

    println!("\n=== import_from_local（读 secretStorage + 验签）===");
    let cred = match adapter.import_from_local(host.as_ref()).await {
        Ok(c) => {
            println!("  uid          = {}", c.uid);
            println!("  display_name = {:?}", c.display_name);
            c
        }
        Err(e) => {
            eprintln!("  导入失败: {e}");
            std::process::exit(1);
        }
    };

    println!("\n=== inspect_quota ===");
    match adapter.inspect_quota(&cred).await {
        Ok(q) => {
            println!("  plan_name    = {:?}", q.plan_name);
            println!("  claim_status = {:?}", q.claim_status);
            for b in &q.buckets {
                println!("  {:<14} {}/{} {}", b.label, b.remaining, b.total, b.unit);
            }
        }
        Err(e) => eprintln!("  查询失败: {e}"),
    }

    println!("\n=== claim_daily（/v1/ops/delivery→claim→confirm 真领）===");
    match adapter.claim_daily(&cred, None).await {
        Ok(r) => println!("  claimed={} summary={}", r.claimed, r.reward_summary),
        Err(e) => eprintln!("  失败: {e}"),
    }
}
