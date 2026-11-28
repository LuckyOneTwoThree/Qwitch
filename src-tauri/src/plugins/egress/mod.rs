use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::{ApiAuthScheme, CredentialEnvelope, PoolMember, WireProtocolKind};
use crate::gateway::ir::IrRequest;

#[async_trait]
pub trait EgressTransport: Send + Sync {
    fn id(&self) -> &str;

    async fn send_stream(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<reqwest::Response, String>;

    async fn call_buffered(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<(u16, String), String>;
}

pub struct StandardHttpEgress {
    id: String,
    default_base_url: String,
    extra_headers: Vec<(String, String)>,
    wire_protocol: Option<WireProtocolKind>,
    auth_scheme: ApiAuthScheme,
    auth_header_name: Option<String>,
    allow_lan_http: bool,
}

impl StandardHttpEgress {
    pub fn new(id: impl Into<String>, default_base_url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            default_base_url: default_base_url.into(),
            extra_headers: Vec::new(),
            wire_protocol: None,
            auth_scheme: ApiAuthScheme::Bearer,
            auth_header_name: None,
            allow_lan_http: false,
        }
    }

    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.extra_headers = headers;
        self
    }

    pub fn with_protocol(mut self, protocol: WireProtocolKind) -> Self {
        self.wire_protocol = Some(protocol);
        self
    }

    pub fn with_auth(mut self, scheme: ApiAuthScheme, header_name: Option<String>) -> Self {
        self.auth_scheme = scheme;
        self.auth_header_name = header_name;
        self
    }

    pub fn with_allow_lan_http(mut self, allow: bool) -> Self {
        self.allow_lan_http = allow;
        self
    }
}

#[async_trait]
impl EgressTransport for StandardHttpEgress {
    fn id(&self) -> &str {
        &self.id
    }

    async fn send_stream(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<reqwest::Response, String> {
        crate::gateway::send_standard_http_stream(
            member,
            cred,
            ir_req,
            upstream_model,
            crate::gateway::StandardHttpOptions {
                default_base: &self.default_base_url,
                plugin_headers: &self.extra_headers,
                explicit_protocol: self.wire_protocol,
                auth_scheme: self.auth_scheme,
                auth_header_name: self.auth_header_name.as_deref(),
                allow_lan_http: self.allow_lan_http,
            },
        )
        .await
    }

    async fn call_buffered(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<(u16, String), String> {
        crate::gateway::call_standard_http_buffered(
            member,
            cred,
            ir_req,
            upstream_model,
            crate::gateway::StandardHttpOptions {
                default_base: &self.default_base_url,
                plugin_headers: &self.extra_headers,
                explicit_protocol: self.wire_protocol,
                auth_scheme: self.auth_scheme,
                auth_header_name: self.auth_header_name.as_deref(),
                allow_lan_http: self.allow_lan_http,
            },
        )
        .await
    }
}

pub struct CodeArtsEgress;
pub struct TraeEgress;
pub struct QoderEgress;
pub struct DoubaoEgress;
pub struct WorkBuddyEgress;
pub struct MiniMaxEgress;

#[async_trait]
impl EgressTransport for CodeArtsEgress {
    fn id(&self) -> &str {
        "codearts"
    }

    async fn send_stream(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<reqwest::Response, String> {
        let client = crate::adapter::common::long_running_http_client();
        let (_, bare) = crate::domain::split_scoped_model_id(upstream_model);
        let wire_model = crate::domain::resolve_vendor_wire_model_id(&member.tool_id, bare);
        crate::gateway::send_codearts_stream(client, member, cred, ir_req, &wire_model).await
    }

    async fn call_buffered(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<(u16, String), String> {
        let resp = self
            .send_stream(member, cred, ir_req, upstream_model)
            .await?;
        let code = resp.status().as_u16();
        let text = crate::gateway::read_bounded_response_text(resp, &member.tool_id).await?;
        Ok((code, text))
    }
}

#[async_trait]
impl EgressTransport for TraeEgress {
    fn id(&self) -> &str {
        "trae_cn"
    }

    async fn send_stream(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<reqwest::Response, String> {
        let client = crate::adapter::common::long_running_http_client();
        let (_, bare) = crate::domain::split_scoped_model_id(upstream_model);
        let wire_model = crate::domain::resolve_vendor_wire_model_id(&member.tool_id, bare);
        crate::gateway::send_trae_stream(client, member, cred, ir_req, &wire_model).await
    }

    async fn call_buffered(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<(u16, String), String> {
        let resp = self
            .send_stream(member, cred, ir_req, upstream_model)
            .await?;
        let code = resp.status().as_u16();
        let text = crate::gateway::read_bounded_response_text(resp, &member.tool_id).await?;
        Ok((code, text))
    }
}

#[async_trait]
impl EgressTransport for QoderEgress {
    fn id(&self) -> &str {
        "qoder_cn"
    }

    async fn send_stream(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<reqwest::Response, String> {
        let client = crate::adapter::common::long_running_http_client();
        let (_, bare) = crate::domain::split_scoped_model_id(upstream_model);
        let wire_model = crate::domain::resolve_vendor_wire_model_id(&member.tool_id, bare);
        crate::gateway::send_qoder_stream(client, member, cred, ir_req, &wire_model).await
    }

    async fn call_buffered(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<(u16, String), String> {
        let resp = self
            .send_stream(member, cred, ir_req, upstream_model)
            .await?;
        let code = resp.status().as_u16();
        let text = crate::gateway::read_bounded_response_text(resp, &member.tool_id).await?;
        Ok((code, text))
    }
}

#[async_trait]
impl EgressTransport for DoubaoEgress {
    fn id(&self) -> &str {
        "doubaowork"
    }

    async fn send_stream(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<reqwest::Response, String> {
        let client = crate::adapter::common::long_running_http_client();
        let (_, bare) = crate::domain::split_scoped_model_id(upstream_model);
        let wire_model = crate::domain::resolve_vendor_wire_model_id(&member.tool_id, bare);
        crate::gateway::doubao::send_doubao_stream(client, member, cred, ir_req, &wire_model).await
    }

    async fn call_buffered(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<(u16, String), String> {
        let resp = self
            .send_stream(member, cred, ir_req, upstream_model)
            .await?;
        let code = resp.status().as_u16();
        let text = crate::gateway::read_bounded_response_text(resp, &member.tool_id).await?;
        Ok((code, text))
    }
}

#[async_trait]
impl EgressTransport for WorkBuddyEgress {
    fn id(&self) -> &str {
        "workbuddy"
    }

    async fn send_stream(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<reqwest::Response, String> {
        crate::gateway::send_standard_http_stream(
            member,
            cred,
            ir_req,
            upstream_model,
            crate::gateway::StandardHttpOptions::default_with_base("https://copilot.tencent.com"),
        )
        .await
    }

    async fn call_buffered(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<(u16, String), String> {
        crate::gateway::call_workbuddy_buffered(member, cred, ir_req, upstream_model).await
    }
}

#[async_trait]
impl EgressTransport for MiniMaxEgress {
    fn id(&self) -> &str {
        "minimax_code_cn"
    }

    async fn send_stream(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<reqwest::Response, String> {
        crate::gateway::send_minimax_stream(member, cred, ir_req, upstream_model).await
    }

    async fn call_buffered(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<(u16, String), String> {
        crate::gateway::call_minimax_buffered(member, cred, ir_req, upstream_model).await
    }
}

pub fn create_builtin_egress_transports() -> HashMap<String, Arc<dyn EgressTransport>> {
    let list: Vec<Arc<dyn EgressTransport>> = vec![
        Arc::new(CodeArtsEgress),
        Arc::new(TraeEgress),
        Arc::new(QoderEgress),
        Arc::new(DoubaoEgress),
        Arc::new(WorkBuddyEgress),
        Arc::new(MiniMaxEgress),
        Arc::new(StandardHttpEgress::new(
            "lobsterai",
            "https://lobsterai-server.youdao.com/api/proxy",
        )),
    ];
    list.into_iter().map(|t| (t.id().to_string(), t)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_all_builtin_egress_transports() {
        let map = create_builtin_egress_transports();
        for id in [
            "trae_cn",
            "workbuddy",
            "qoder_cn",
            "minimax_code_cn",
            "lobsterai",
            "doubaowork",
            "codearts",
        ] {
            assert!(
                map.contains_key(id),
                "missing builtin egress transport {id}"
            );
            assert_eq!(map[id].id(), id);
        }
    }
}
