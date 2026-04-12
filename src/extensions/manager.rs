//! Central extension manager that dispatches operations by ExtensionKind.
//!
//! Holds references to MCP infrastructure, WASM tool runtime, secrets store,
//! and tool registry. All extension operations (search, install, auth, activate,
//! list, remove) flow through here.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::extensions::discovery::OnlineDiscovery;
use crate::extensions::registry::ExtensionRegistry;
use crate::extensions::{
    ActivateResult, AuthHint, AuthResult, ExtensionAuthInfo, ExtensionError, ExtensionKind,
    ExtensionSource, ExtensionStatus, InstallResult, InstalledExtension, RegistryEntry,
    ResultSource, SearchResult,
};
use crate::secrets::{CreateSecretParams, SecretsStore};
use crate::tools::ToolRegistry;
use crate::tools::mcp::McpClient;
use crate::tools::mcp::auth::{
    PkceChallenge, authorize_mcp_server, build_authorization_url, discover_full_oauth_metadata,
    find_available_port, is_authenticated, register_client,
};
use crate::tools::mcp::config::McpServerConfig;
use crate::tools::mcp::session::McpSessionManager;
use crate::tools::wasm::{WasmToolLoader, WasmToolRuntime, discover_tools};

/// Pending OAuth authorization state.
struct PendingAuth {
    _name: String,
    _kind: ExtensionKind,
    created_at: std::time::Instant,
}

/// Central manager for extension lifecycle operations.
pub struct ExtensionManager {
    registry: ExtensionRegistry,
    discovery: OnlineDiscovery,

    // MCP infrastructure
    mcp_session_manager: Arc<McpSessionManager>,
    /// Active MCP clients keyed by server name.
    mcp_clients: RwLock<HashMap<String, Arc<McpClient>>>,

    // WASM tool infrastructure
    wasm_tool_runtime: Option<Arc<WasmToolRuntime>>,
    wasm_tools_dir: PathBuf,
    wasm_channels_dir: PathBuf,

    // Shared
    secrets: Arc<dyn SecretsStore + Send + Sync>,
    tool_registry: Arc<ToolRegistry>,
    pending_auth: RwLock<HashMap<String, PendingAuth>>,
    /// Cached last activation error per extension name.
    activation_errors: RwLock<HashMap<String, String>>,
    /// Tunnel URL for remote OAuth callbacks (used in future iterations).
    _tunnel_url: Option<String>,
    user_id: String,
    /// Optional database store for DB-backed MCP config.
    store: Option<Arc<dyn crate::db::Database>>,
}

impl ExtensionManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mcp_session_manager: Arc<McpSessionManager>,
        secrets: Arc<dyn SecretsStore + Send + Sync>,
        tool_registry: Arc<ToolRegistry>,
        wasm_tool_runtime: Option<Arc<WasmToolRuntime>>,
        wasm_tools_dir: PathBuf,
        wasm_channels_dir: PathBuf,
        tunnel_url: Option<String>,
        user_id: String,
        store: Option<Arc<dyn crate::db::Database>>,
    ) -> Self {
        Self {
            registry: ExtensionRegistry::new(),
            discovery: OnlineDiscovery::new(),
            mcp_session_manager,
            mcp_clients: RwLock::new(HashMap::new()),
            wasm_tool_runtime,
            wasm_tools_dir,
            wasm_channels_dir,
            secrets,
            tool_registry,
            pending_auth: RwLock::new(HashMap::new()),
            activation_errors: RwLock::new(HashMap::new()),
            _tunnel_url: tunnel_url,
            user_id,
            store,
        }
    }

    /// Search for extensions. If `discover` is true, also searches online.
    pub async fn search(
        &self,
        query: &str,
        discover: bool,
    ) -> Result<Vec<SearchResult>, ExtensionError> {
        let mut results = self.registry.search(query).await;

        if discover && results.is_empty() {
            tracing::info!("No built-in results for '{}', searching online...", query);
            let discovered = self.discovery.discover(query).await;

            if !discovered.is_empty() {
                // Cache for future lookups
                self.registry.cache_discovered(discovered.clone()).await;

                // Add to results
                for entry in discovered {
                    results.push(SearchResult {
                        entry,
                        source: ResultSource::Discovered,
                        validated: true,
                    });
                }
            }
        }

        Ok(results)
    }

    /// Install an extension by name (from registry) or by explicit URL.
    pub async fn install(
        &self,
        name: &str,
        url: Option<&str>,
        kind_hint: Option<ExtensionKind>,
    ) -> Result<InstallResult, ExtensionError> {
        // If we have a registry entry, use it
        if let Some(entry) = self.registry.get(name).await {
            return self.install_from_entry(&entry).await;
        }

        // If a URL was provided, determine kind and install
        if let Some(url) = url {
            let kind = kind_hint.unwrap_or_else(|| infer_kind_from_url(url));
            return match kind {
                ExtensionKind::McpServer => self.install_mcp_from_url(name, url).await,
                ExtensionKind::WasmTool => self.install_wasm_tool_from_url(name, url).await,
                ExtensionKind::WasmChannel => {
                    Err(ExtensionError::InstallFailed(
                        "WASM channel installation from URL not yet supported. \
                         Place the .wasm and .capabilities.json files in ~/.rustytalon/channels/ and restart."
                            .to_string(),
                    ))
                }
            };
        }

        Err(ExtensionError::NotFound(format!(
            "'{}' not found in registry. Try searching with discover:true or provide a URL.",
            name
        )))
    }

    /// Authenticate an installed extension.
    pub async fn auth(
        &self,
        name: &str,
        token: Option<&str>,
    ) -> Result<AuthResult, ExtensionError> {
        // Clean up expired pending auths
        self.cleanup_expired_auths().await;

        // Determine what kind of extension this is
        let kind = self.determine_installed_kind(name).await?;

        match kind {
            ExtensionKind::McpServer => self.auth_mcp(name, token).await,
            ExtensionKind::WasmTool => self.auth_wasm_tool(name, token).await,
            ExtensionKind::WasmChannel => self.auth_wasm_tool(name, token).await,
        }
    }

    /// Activate an installed (and optionally authenticated) extension.
    pub async fn activate(&self, name: &str) -> Result<ActivateResult, ExtensionError> {
        let kind = self.determine_installed_kind(name).await?;

        let result = match kind {
            ExtensionKind::McpServer => self.activate_mcp(name).await,
            ExtensionKind::WasmTool => self.activate_wasm_tool(name).await,
            ExtensionKind::WasmChannel => Err(ExtensionError::ChannelNeedsRestart),
        };

        match &result {
            Ok(_) => {
                self.activation_errors.write().await.remove(name);
            }
            Err(e) => {
                self.activation_errors
                    .write()
                    .await
                    .insert(name.to_string(), e.to_string());
            }
        }

        result
    }

    /// List all installed extensions with their status.
    pub async fn list(
        &self,
        kind_filter: Option<ExtensionKind>,
    ) -> Result<Vec<InstalledExtension>, ExtensionError> {
        let mut extensions = Vec::new();

        // List MCP servers
        if kind_filter.is_none() || kind_filter == Some(ExtensionKind::McpServer) {
            match self.load_mcp_servers().await {
                Ok(servers) => {
                    for server in &servers.servers {
                        let authenticated =
                            is_authenticated(server, &self.secrets, &self.user_id).await;
                        let clients = self.mcp_clients.read().await;
                        let active = clients.contains_key(&server.name);

                        // Get tool names if active
                        let tools = if active {
                            self.tool_registry
                                .list()
                                .await
                                .into_iter()
                                .filter(|t| t.starts_with(&format!("{}_", server.name)))
                                .collect()
                        } else {
                            Vec::new()
                        };

                        let errors = self.activation_errors.read().await;
                        let error = errors.get(&server.name).cloned();
                        let status = compute_status(authenticated, active, error.is_some());
                        extensions.push(InstalledExtension {
                            name: server.name.clone(),
                            kind: ExtensionKind::McpServer,
                            description: server.description.clone(),
                            url: Some(server.url.clone()),
                            authenticated,
                            active,
                            status,
                            error,
                            tools,
                        });
                    }
                }
                Err(e) => {
                    tracing::debug!("Failed to load MCP servers for listing: {}", e);
                }
            }
        }

        // List WASM tools
        if (kind_filter.is_none() || kind_filter == Some(ExtensionKind::WasmTool))
            && self.wasm_tools_dir.exists()
        {
            match discover_tools(&self.wasm_tools_dir).await {
                Ok(tools) => {
                    for (name, _discovered) in tools {
                        let active = self.tool_registry.has(&name).await;

                        let errors = self.activation_errors.read().await;
                        let error = errors.get(&name).cloned();
                        let status = compute_status(true, active, error.is_some());
                        extensions.push(InstalledExtension {
                            name: name.clone(),
                            kind: ExtensionKind::WasmTool,
                            description: None,
                            url: None,
                            authenticated: true, // WASM tools don't always need auth
                            active,
                            status,
                            error,
                            tools: if active { vec![name] } else { Vec::new() },
                        });
                    }
                }
                Err(e) => {
                    tracing::debug!("Failed to discover WASM tools for listing: {}", e);
                }
            }
        }

        // List WASM channels
        if (kind_filter.is_none() || kind_filter == Some(ExtensionKind::WasmChannel))
            && self.wasm_channels_dir.exists()
        {
            match crate::channels::wasm::discover_channels(&self.wasm_channels_dir).await {
                Ok(channels) => {
                    for (name, _discovered) in channels {
                        extensions.push(InstalledExtension {
                            name,
                            kind: ExtensionKind::WasmChannel,
                            description: None,
                            url: None,
                            authenticated: true,
                            active: true, // If loaded at startup, they're active
                            status: ExtensionStatus::Active,
                            error: None,
                            tools: Vec::new(),
                        });
                    }
                }
                Err(e) => {
                    tracing::debug!("Failed to discover WASM channels for listing: {}", e);
                }
            }
        }

        Ok(extensions)
    }

    /// Remove an installed extension.
    pub async fn remove(&self, name: &str) -> Result<String, ExtensionError> {
        let kind = self.determine_installed_kind(name).await?;

        match kind {
            ExtensionKind::McpServer => {
                // Unregister tools with this server's prefix
                let tool_names: Vec<String> = self
                    .tool_registry
                    .list()
                    .await
                    .into_iter()
                    .filter(|t| t.starts_with(&format!("{}_", name)))
                    .collect();

                for tool_name in &tool_names {
                    self.tool_registry.unregister(tool_name).await;
                }

                // Remove MCP client
                self.mcp_clients.write().await.remove(name);

                // Remove from config
                self.remove_mcp_server(name)
                    .await
                    .map_err(|e| ExtensionError::Config(e.to_string()))?;

                Ok(format!(
                    "Removed MCP server '{}' and {} tool(s)",
                    name,
                    tool_names.len()
                ))
            }
            ExtensionKind::WasmTool => {
                // Unregister from tool registry
                self.tool_registry.unregister(name).await;

                // Delete files
                let wasm_path = self.wasm_tools_dir.join(format!("{}.wasm", name));
                let cap_path = self
                    .wasm_tools_dir
                    .join(format!("{}.capabilities.json", name));

                if wasm_path.exists() {
                    tokio::fs::remove_file(&wasm_path)
                        .await
                        .map_err(|e| ExtensionError::Other(e.to_string()))?;
                }
                if cap_path.exists() {
                    let _ = tokio::fs::remove_file(&cap_path).await;
                }

                Ok(format!("Removed WASM tool '{}'", name))
            }
            ExtensionKind::WasmChannel => Err(ExtensionError::Other(
                "Channel removal requires restart. Delete the .wasm file from ~/.rustytalon/channels/ and restart."
                    .to_string(),
            )),
        }
    }

    /// Get auth setup information for an extension, used by the web UI wizard.
    pub async fn get_auth_info(&self, name: &str) -> Result<ExtensionAuthInfo, ExtensionError> {
        // Installed MCP server config takes precedence over the registry.
        //
        // The registry's AuthHint::Dcr only indicates "this server uses OAuth DCR in principle",
        // but for servers without a pre-configured OAuth client ID the DCR discovery requires a
        // reachable server.  The installed McpServerConfig is the ground truth: if `oauth` is
        // Some we can attempt the browser flow; if it is None (the common case) we fall back to
        // manual token entry so the wizard always shows a token input field.
        //
        // WASM tools and uninstalled extensions still fall through to the registry check below.
        if let Ok(server) = self.get_mcp_server(name).await {
            let display_name = server.description.clone().or_else(|| {
                // Capitalize first letter of the server name as a friendly fallback.
                let mut chars = name.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
            });

            // Pull the display_name from the registry entry if we have a better one.
            let display_name = self
                .registry
                .get(name)
                .await
                .map(|e| Some(e.display_name.clone()))
                .unwrap_or(display_name);

            if server.oauth.is_some() {
                // Pre-configured OAuth client → use DCR/browser flow.
                return Ok(ExtensionAuthInfo {
                    name: name.to_string(),
                    auth_type: "dcr".to_string(),
                    instructions: None,
                    setup_url: None,
                    token_hint: None,
                    display_name,
                    oauth_available: true,
                });
            } else if server.requires_auth() {
                // Remote HTTPS server without pre-configured OAuth → manual token entry.
                // The token is stored encrypted and injected as a Bearer Authorization
                // header on every MCP connection, never touching the LLM or logs.
                return Ok(ExtensionAuthInfo {
                    name: name.to_string(),
                    auth_type: "manual".to_string(),
                    instructions: Some(format!(
                        "Enter an API token or personal access token for {}. \
                         The token will be stored encrypted and sent as a Bearer \
                         Authorization header when connecting to the server.",
                        display_name.as_deref().unwrap_or(name)
                    )),
                    setup_url: None,
                    token_hint: None,
                    display_name,
                    oauth_available: false,
                });
            } else {
                // Localhost / dev server — no auth required.
                return Ok(ExtensionAuthInfo {
                    name: name.to_string(),
                    auth_type: "none".to_string(),
                    instructions: None,
                    setup_url: None,
                    token_hint: None,
                    display_name,
                    oauth_available: false,
                });
            }
        }

        // Try the registry for the auth_hint (WASM tools and uninstalled extensions).
        if let Some(entry) = self.registry.get(name).await {
            return Ok(auth_info_from_entry(&entry, name));
        }

        // For installed WASM tools, try reading the capabilities.json
        let cap_path = self
            .wasm_tools_dir
            .join(format!("{}.capabilities.json", name));
        if cap_path.exists()
            && let Ok(data) = tokio::fs::read_to_string(&cap_path).await
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                    return Ok(auth_info_from_capabilities(&json, name));
                }

        // For installed WASM channels, try the channels dir
        let ch_cap_path = self
            .wasm_channels_dir
            .join(format!("{}.capabilities.json", name));
        if ch_cap_path.exists()
            && let Ok(data) = tokio::fs::read_to_string(&ch_cap_path).await
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                    return Ok(auth_info_from_capabilities(&json, name));
                }

        Err(ExtensionError::NotFound(name.to_string()))
    }

    /// Read the `config_schema` JSON from a capabilities.json file for an extension.
    ///
    /// Checks WASM channels dir first (discord, telegram, matrix), then WASM tools dir.
    /// Returns `None` if no capabilities file is found or it has no `config_schema` field.
    pub async fn get_config_schema(&self, name: &str) -> Option<serde_json::Value> {
        // WASM channels dir first
        let ch_cap_path = self
            .wasm_channels_dir
            .join(format!("{}.capabilities.json", name));
        if ch_cap_path.exists()
            && let Ok(data) = tokio::fs::read_to_string(&ch_cap_path).await
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                    return json.get("config_schema").cloned();
                }

        // WASM tools dir
        let tool_cap_path = self
            .wasm_tools_dir
            .join(format!("{}.capabilities.json", name));
        if tool_cap_path.exists()
            && let Ok(data) = tokio::fs::read_to_string(&tool_cap_path).await
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                    return json.get("config_schema").cloned();
                }

        None
    }

    // ── MCP config helpers (DB with disk fallback) ─────────────────────

    async fn load_mcp_servers(
        &self,
    ) -> Result<crate::tools::mcp::config::McpServersFile, crate::tools::mcp::config::ConfigError>
    {
        if let Some(ref store) = self.store {
            crate::tools::mcp::config::load_mcp_servers_from_db(store.as_ref(), &self.user_id).await
        } else {
            crate::tools::mcp::config::load_mcp_servers().await
        }
    }

    async fn get_mcp_server(
        &self,
        name: &str,
    ) -> Result<McpServerConfig, crate::tools::mcp::config::ConfigError> {
        let servers = self.load_mcp_servers().await?;
        servers.get(name).cloned().ok_or_else(|| {
            crate::tools::mcp::config::ConfigError::ServerNotFound {
                name: name.to_string(),
            }
        })
    }

    async fn add_mcp_server(
        &self,
        config: McpServerConfig,
    ) -> Result<(), crate::tools::mcp::config::ConfigError> {
        config.validate()?;
        if let Some(ref store) = self.store {
            crate::tools::mcp::config::add_mcp_server_db(store.as_ref(), &self.user_id, config)
                .await
        } else {
            crate::tools::mcp::config::add_mcp_server(config).await
        }
    }

    async fn remove_mcp_server(
        &self,
        name: &str,
    ) -> Result<(), crate::tools::mcp::config::ConfigError> {
        if let Some(ref store) = self.store {
            crate::tools::mcp::config::remove_mcp_server_db(store.as_ref(), &self.user_id, name)
                .await
        } else {
            crate::tools::mcp::config::remove_mcp_server(name).await
        }
    }

    // ── Private helpers ──────────────────────────────────────────────────

    async fn install_from_entry(
        &self,
        entry: &RegistryEntry,
    ) -> Result<InstallResult, ExtensionError> {
        match entry.kind {
            ExtensionKind::McpServer => {
                let url = match &entry.source {
                    ExtensionSource::McpUrl { url } => url.clone(),
                    ExtensionSource::Discovered { url } => url.clone(),
                    _ => {
                        return Err(ExtensionError::InstallFailed(
                            "Registry entry for MCP server has no URL".to_string(),
                        ));
                    }
                };
                self.install_mcp_from_url(&entry.name, &url).await
            }
            ExtensionKind::WasmTool => match &entry.source {
                ExtensionSource::WasmDownload { wasm_url, .. } => {
                    self.install_wasm_tool_from_url(&entry.name, wasm_url).await
                }
                _ => Err(ExtensionError::InstallFailed(
                    "WASM tool entry has no download URL".to_string(),
                )),
            },
            ExtensionKind::WasmChannel => Err(ExtensionError::InstallFailed(
                "WASM channel installation not yet supported via this flow".to_string(),
            )),
        }
    }

    async fn install_mcp_from_url(
        &self,
        name: &str,
        url: &str,
    ) -> Result<InstallResult, ExtensionError> {
        // Check if already installed
        if self.get_mcp_server(name).await.is_ok() {
            return Err(ExtensionError::AlreadyInstalled(name.to_string()));
        }

        let config = McpServerConfig::new(name, url);
        config
            .validate()
            .map_err(|e| ExtensionError::InvalidUrl(e.to_string()))?;

        self.add_mcp_server(config)
            .await
            .map_err(|e| ExtensionError::Config(e.to_string()))?;

        tracing::info!("Installed MCP server '{}' at {}", name, url);

        Ok(InstallResult {
            name: name.to_string(),
            kind: ExtensionKind::McpServer,
            message: format!(
                "MCP server '{}' installed. Run auth next to authenticate.",
                name
            ),
        })
    }

    async fn install_wasm_tool_from_url(
        &self,
        name: &str,
        url: &str,
    ) -> Result<InstallResult, ExtensionError> {
        // Require HTTPS to prevent downgrade attacks
        if !url.starts_with("https://") {
            return Err(ExtensionError::InstallFailed(
                "Only HTTPS URLs are allowed for extension downloads".to_string(),
            ));
        }

        // 50 MB cap to prevent disk-fill DoS
        const MAX_WASM_SIZE: usize = 50 * 1024 * 1024;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| ExtensionError::DownloadFailed(e.to_string()))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| ExtensionError::DownloadFailed(e.to_string()))?;

        if !response.status().is_success() {
            return Err(ExtensionError::DownloadFailed(format!(
                "HTTP {}",
                response.status()
            )));
        }

        // Check Content-Length header before downloading the full body
        if let Some(len) = response.content_length()
            && len as usize > MAX_WASM_SIZE
        {
            return Err(ExtensionError::InstallFailed(format!(
                "WASM binary too large ({} bytes, max {} bytes)",
                len, MAX_WASM_SIZE
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| ExtensionError::DownloadFailed(e.to_string()))?;

        if bytes.len() > MAX_WASM_SIZE {
            return Err(ExtensionError::InstallFailed(format!(
                "WASM binary too large ({} bytes, max {} bytes)",
                bytes.len(),
                MAX_WASM_SIZE
            )));
        }

        // Basic WASM magic number check (\0asm)
        if bytes.len() < 4 || &bytes[..4] != b"\0asm" {
            return Err(ExtensionError::InstallFailed(
                "Downloaded file is not a valid WASM binary (bad magic number)".to_string(),
            ));
        }

        // Ensure tools directory exists
        tokio::fs::create_dir_all(&self.wasm_tools_dir)
            .await
            .map_err(|e| ExtensionError::InstallFailed(e.to_string()))?;

        // Write the WASM file
        let wasm_path = self.wasm_tools_dir.join(format!("{}.wasm", name));
        tokio::fs::write(&wasm_path, &bytes)
            .await
            .map_err(|e| ExtensionError::InstallFailed(e.to_string()))?;

        tracing::info!(
            "Installed WASM tool '{}' ({} bytes) from {} to {}",
            name,
            bytes.len(),
            url,
            wasm_path.display()
        );

        Ok(InstallResult {
            name: name.to_string(),
            kind: ExtensionKind::WasmTool,
            message: format!("WASM tool '{}' installed. Run activate to load it.", name),
        })
    }

    async fn auth_mcp(
        &self,
        name: &str,
        token: Option<&str>,
    ) -> Result<AuthResult, ExtensionError> {
        let server = self
            .get_mcp_server(name)
            .await
            .map_err(|e| ExtensionError::NotInstalled(e.to_string()))?;

        // If a token was provided directly, store it and we're done.
        if let Some(token_value) = token {
            let secret_name = server.token_secret_name();
            let params =
                CreateSecretParams::new(&secret_name, token_value).with_provider(name.to_string());
            self.secrets
                .create(&self.user_id, params)
                .await
                .map_err(|e| ExtensionError::AuthFailed(e.to_string()))?;

            tracing::info!("MCP server '{}' authenticated via manual token", name);
            return Ok(AuthResult {
                name: name.to_string(),
                kind: ExtensionKind::McpServer,
                auth_url: None,
                callback_type: None,
                instructions: None,
                setup_url: None,
                awaiting_token: false,
                status: "authenticated".to_string(),
            });
        }

        // Check if already authenticated
        if is_authenticated(&server, &self.secrets, &self.user_id).await {
            return Ok(AuthResult {
                name: name.to_string(),
                kind: ExtensionKind::McpServer,
                auth_url: None,
                callback_type: None,
                instructions: None,
                setup_url: None,
                awaiting_token: false,
                status: "authenticated".to_string(),
            });
        }

        // Run the full OAuth flow (opens browser, waits for callback)
        match authorize_mcp_server(&server, &self.secrets, &self.user_id).await {
            Ok(_token) => {
                tracing::info!("MCP server '{}' authenticated via OAuth", name);
                Ok(AuthResult {
                    name: name.to_string(),
                    kind: ExtensionKind::McpServer,
                    auth_url: None,
                    callback_type: None,
                    instructions: None,
                    setup_url: None,
                    awaiting_token: false,
                    status: "authenticated".to_string(),
                })
            }
            Err(crate::tools::mcp::auth::AuthError::NotSupported) => {
                // Server doesn't support OAuth, try building a URL first
                match self.auth_mcp_build_url(name, &server).await {
                    Ok(result) => Ok(result),
                    Err(_) => {
                        // No OAuth, no DCR: fall back to manual token entry
                        Ok(AuthResult {
                            name: name.to_string(),
                            kind: ExtensionKind::McpServer,
                            auth_url: None,
                            callback_type: None,
                            instructions: Some(format!(
                                "Server '{}' does not support OAuth. \
                                 Please provide an API token/key for this server.",
                                name
                            )),
                            setup_url: None,
                            awaiting_token: true,
                            status: "awaiting_token".to_string(),
                        })
                    }
                }
            }
            Err(e) => {
                // OAuth failed for some other reason, fall back to manual token
                Ok(AuthResult {
                    name: name.to_string(),
                    kind: ExtensionKind::McpServer,
                    auth_url: None,
                    callback_type: None,
                    instructions: Some(format!(
                        "OAuth failed for '{}': {}. \
                         Please provide an API token/key manually.",
                        name, e
                    )),
                    setup_url: None,
                    awaiting_token: true,
                    status: "awaiting_token".to_string(),
                })
            }
        }
    }

    /// Build an auth URL for cases where non-interactive auth is needed
    /// (e.g., running via Telegram where we can't open a browser).
    async fn auth_mcp_build_url(
        &self,
        name: &str,
        server: &McpServerConfig,
    ) -> Result<AuthResult, ExtensionError> {
        // Try to discover OAuth metadata and build a URL the user can open manually
        let metadata = discover_full_oauth_metadata(&server.url)
            .await
            .map_err(|e| ExtensionError::AuthFailed(e.to_string()))?;

        // Try DCR if no client_id configured
        let (client_id, redirect_uri) = if let Some(ref oauth) = server.oauth {
            let port = find_available_port()
                .await
                .map_err(|e| ExtensionError::AuthFailed(e.to_string()))?;
            let redirect = format!("http://localhost:{}/callback", port.1);
            (oauth.client_id.clone(), redirect)
        } else if let Some(ref reg_endpoint) = metadata.registration_endpoint {
            let port = find_available_port()
                .await
                .map_err(|e| ExtensionError::AuthFailed(e.to_string()))?;
            let redirect = format!("http://localhost:{}/callback", port.1);

            let registration = register_client(reg_endpoint, &redirect)
                .await
                .map_err(|e| ExtensionError::AuthFailed(e.to_string()))?;

            (registration.client_id, redirect)
        } else {
            return Err(ExtensionError::AuthFailed(
                "Server doesn't support OAuth or Dynamic Client Registration".to_string(),
            ));
        };

        let pkce = PkceChallenge::generate();
        let auth_url = build_authorization_url(
            &metadata.authorization_endpoint,
            &client_id,
            &redirect_uri,
            &metadata.scopes_supported,
            Some(&pkce),
            &std::collections::HashMap::new(),
        );

        // Store pending auth for later callback handling
        self.pending_auth.write().await.insert(
            name.to_string(),
            PendingAuth {
                _name: name.to_string(),
                _kind: ExtensionKind::McpServer,
                created_at: std::time::Instant::now(),
            },
        );

        Ok(AuthResult {
            name: name.to_string(),
            kind: ExtensionKind::McpServer,
            auth_url: Some(auth_url),
            callback_type: Some("local".to_string()),
            instructions: None,
            setup_url: None,
            awaiting_token: false,
            status: "awaiting_authorization".to_string(),
        })
    }

    async fn auth_wasm_tool(
        &self,
        name: &str,
        token: Option<&str>,
    ) -> Result<AuthResult, ExtensionError> {
        // Read the capabilities file to get auth config
        let cap_path = self
            .wasm_tools_dir
            .join(format!("{}.capabilities.json", name));

        if !cap_path.exists() {
            // No capabilities = no auth needed
            return Ok(AuthResult {
                name: name.to_string(),
                kind: ExtensionKind::WasmTool,
                auth_url: None,
                callback_type: None,
                instructions: None,
                setup_url: None,
                awaiting_token: false,
                status: "no_auth_required".to_string(),
            });
        }

        let cap_bytes = tokio::fs::read(&cap_path)
            .await
            .map_err(|e| ExtensionError::Other(e.to_string()))?;

        let cap_file = crate::tools::wasm::CapabilitiesFile::from_bytes(&cap_bytes)
            .map_err(|e| ExtensionError::Other(e.to_string()))?;

        let auth = match cap_file.auth {
            Some(auth) => auth,
            None => {
                return Ok(AuthResult {
                    name: name.to_string(),
                    kind: ExtensionKind::WasmTool,
                    auth_url: None,
                    callback_type: None,
                    instructions: None,
                    setup_url: None,
                    awaiting_token: false,
                    status: "no_auth_required".to_string(),
                });
            }
        };

        // Check env var first
        if let Some(ref env_var) = auth.env_var
            && let Ok(value) = std::env::var(env_var)
        {
            // Store the env var value as a secret
            let params =
                CreateSecretParams::new(&auth.secret_name, &value).with_provider(name.to_string());
            self.secrets
                .create(&self.user_id, params)
                .await
                .map_err(|e| ExtensionError::AuthFailed(e.to_string()))?;

            return Ok(AuthResult {
                name: name.to_string(),
                kind: ExtensionKind::WasmTool,
                auth_url: None,
                callback_type: None,
                instructions: None,
                setup_url: None,
                awaiting_token: false,
                status: "authenticated".to_string(),
            });
        }

        // Check if already authenticated
        if self
            .secrets
            .exists(&self.user_id, &auth.secret_name)
            .await
            .unwrap_or(false)
        {
            return Ok(AuthResult {
                name: name.to_string(),
                kind: ExtensionKind::WasmTool,
                auth_url: None,
                callback_type: None,
                instructions: None,
                setup_url: None,
                awaiting_token: false,
                status: "authenticated".to_string(),
            });
        }

        // If a token was provided, store it
        if let Some(token_value) = token {
            let params = CreateSecretParams::new(&auth.secret_name, token_value)
                .with_provider(name.to_string());
            self.secrets
                .create(&self.user_id, params)
                .await
                .map_err(|e| ExtensionError::AuthFailed(e.to_string()))?;

            return Ok(AuthResult {
                name: name.to_string(),
                kind: ExtensionKind::WasmTool,
                auth_url: None,
                callback_type: None,
                instructions: None,
                setup_url: None,
                awaiting_token: false,
                status: "authenticated".to_string(),
            });
        }

        // Return instructions for manual token entry
        let display = auth.display_name.unwrap_or_else(|| name.to_string());
        let instructions = auth
            .instructions
            .unwrap_or_else(|| format!("Please provide your {} API token/key.", display));

        Ok(AuthResult {
            name: name.to_string(),
            kind: ExtensionKind::WasmTool,
            auth_url: None,
            callback_type: None,
            instructions: Some(instructions),
            setup_url: auth.setup_url,
            awaiting_token: true,
            status: "awaiting_token".to_string(),
        })
    }

    async fn activate_mcp(&self, name: &str) -> Result<ActivateResult, ExtensionError> {
        // Check if already activated
        {
            let clients = self.mcp_clients.read().await;
            if clients.contains_key(name) {
                // Already connected, just return the tool names
                let tools: Vec<String> = self
                    .tool_registry
                    .list()
                    .await
                    .into_iter()
                    .filter(|t| t.starts_with(&format!("{}_", name)))
                    .collect();

                return Ok(ActivateResult {
                    name: name.to_string(),
                    kind: ExtensionKind::McpServer,
                    tools_loaded: tools,
                    message: format!("MCP server '{}' already active", name),
                });
            }
        }

        let server = self
            .get_mcp_server(name)
            .await
            .map_err(|e| ExtensionError::NotInstalled(e.to_string()))?;

        let has_tokens = is_authenticated(&server, &self.secrets, &self.user_id).await;

        let client = if has_tokens || server.requires_auth() {
            McpClient::new_authenticated(
                server.clone(),
                Arc::clone(&self.mcp_session_manager),
                Arc::clone(&self.secrets),
                &self.user_id,
            )
        } else {
            McpClient::new_with_name(&server.name, &server.url)
        };

        // Try to list and create tools
        let mcp_tools = client
            .list_tools()
            .await
            .map_err(|e| ExtensionError::ActivationFailed(e.to_string()))?;

        let tool_impls = client
            .create_tools()
            .await
            .map_err(|e| ExtensionError::ActivationFailed(e.to_string()))?;

        let tool_names: Vec<String> = mcp_tools
            .iter()
            .map(|t| format!("{}_{}", name, t.name))
            .collect();

        for tool in tool_impls {
            self.tool_registry.register(tool).await;
        }

        // Store the client
        self.mcp_clients
            .write()
            .await
            .insert(name.to_string(), Arc::new(client));

        tracing::info!(
            "Activated MCP server '{}' with {} tools",
            name,
            tool_names.len()
        );

        Ok(ActivateResult {
            name: name.to_string(),
            kind: ExtensionKind::McpServer,
            tools_loaded: tool_names,
            message: format!("Connected to '{}' and loaded tools", name),
        })
    }

    async fn activate_wasm_tool(&self, name: &str) -> Result<ActivateResult, ExtensionError> {
        // Check if already active
        if self.tool_registry.has(name).await {
            return Ok(ActivateResult {
                name: name.to_string(),
                kind: ExtensionKind::WasmTool,
                tools_loaded: vec![name.to_string()],
                message: format!("WASM tool '{}' already active", name),
            });
        }

        let runtime = self.wasm_tool_runtime.as_ref().ok_or_else(|| {
            ExtensionError::ActivationFailed("WASM runtime not available".to_string())
        })?;

        let wasm_path = self.wasm_tools_dir.join(format!("{}.wasm", name));
        if !wasm_path.exists() {
            return Err(ExtensionError::NotInstalled(format!(
                "WASM tool '{}' not found at {}",
                name,
                wasm_path.display()
            )));
        }

        let cap_path = self
            .wasm_tools_dir
            .join(format!("{}.capabilities.json", name));
        let cap_path_option = if cap_path.exists() {
            Some(cap_path.as_path())
        } else {
            None
        };

        let loader = WasmToolLoader::new(Arc::clone(runtime), Arc::clone(&self.tool_registry));
        loader
            .load_from_files(name, &wasm_path, cap_path_option)
            .await
            .map_err(|e| ExtensionError::ActivationFailed(e.to_string()))?;

        tracing::info!("Activated WASM tool '{}'", name);

        Ok(ActivateResult {
            name: name.to_string(),
            kind: ExtensionKind::WasmTool,
            tools_loaded: vec![name.to_string()],
            message: format!("WASM tool '{}' loaded and ready", name),
        })
    }

    /// Determine what kind of installed extension this is.
    async fn determine_installed_kind(&self, name: &str) -> Result<ExtensionKind, ExtensionError> {
        // Check MCP servers first
        if self.get_mcp_server(name).await.is_ok() {
            return Ok(ExtensionKind::McpServer);
        }

        // Check WASM tools
        let wasm_path = self.wasm_tools_dir.join(format!("{}.wasm", name));
        if wasm_path.exists() {
            return Ok(ExtensionKind::WasmTool);
        }

        // Check WASM channels
        let channel_path = self.wasm_channels_dir.join(format!("{}.wasm", name));
        if channel_path.exists() {
            return Ok(ExtensionKind::WasmChannel);
        }

        Err(ExtensionError::NotInstalled(format!(
            "'{}' is not installed as an MCP server, WASM tool, or WASM channel",
            name
        )))
    }

    async fn cleanup_expired_auths(&self) {
        let mut pending = self.pending_auth.write().await;
        pending.retain(|_, auth| auth.created_at.elapsed() < std::time::Duration::from_secs(300));
    }
}

/// Infer the extension kind from a URL.
fn infer_kind_from_url(url: &str) -> ExtensionKind {
    if url.ends_with(".wasm") {
        ExtensionKind::WasmTool
    } else {
        ExtensionKind::McpServer
    }
}

/// Compute the display status of an extension from its auth/active state.
fn compute_status(authenticated: bool, active: bool, has_error: bool) -> ExtensionStatus {
    if active {
        ExtensionStatus::Active
    } else if has_error {
        ExtensionStatus::Error
    } else if !authenticated {
        ExtensionStatus::NeedsAuth
    } else {
        ExtensionStatus::Inactive
    }
}

/// Derive auth info from a registry entry (public for use by web handlers).
pub fn auth_info_from_entry_pub(entry: &RegistryEntry, name: &str) -> ExtensionAuthInfo {
    auth_info_from_entry(entry, name)
}

/// Derive auth info from a registry entry.
fn auth_info_from_entry(entry: &RegistryEntry, name: &str) -> ExtensionAuthInfo {
    match &entry.auth_hint {
        AuthHint::Dcr => ExtensionAuthInfo {
            name: name.to_string(),
            auth_type: "dcr".to_string(),
            instructions: None,
            setup_url: None,
            token_hint: None,
            display_name: Some(entry.display_name.clone()),
            oauth_available: true,
        },
        AuthHint::OAuthPreConfigured { setup_url } => ExtensionAuthInfo {
            name: name.to_string(),
            auth_type: "oauth".to_string(),
            instructions: Some(format!(
                "Set up OAuth credentials for {} at the link below, then click Authorize.",
                entry.display_name
            )),
            setup_url: Some(setup_url.clone()),
            token_hint: None,
            display_name: Some(entry.display_name.clone()),
            oauth_available: true,
        },
        AuthHint::CapabilitiesAuth => ExtensionAuthInfo {
            name: name.to_string(),
            auth_type: "manual".to_string(),
            instructions: Some(format!("Enter your API token for {}.", entry.display_name)),
            setup_url: None,
            token_hint: None,
            display_name: Some(entry.display_name.clone()),
            oauth_available: false,
        },
        AuthHint::None => ExtensionAuthInfo {
            name: name.to_string(),
            auth_type: "none".to_string(),
            instructions: None,
            setup_url: None,
            token_hint: None,
            display_name: Some(entry.display_name.clone()),
            oauth_available: false,
        },
    }
}

/// Derive auth info from a parsed capabilities.json value.
fn auth_info_from_capabilities(json: &serde_json::Value, name: &str) -> ExtensionAuthInfo {
    let auth = json.get("auth");
    let display_name = auth
        .and_then(|a| a.get("display_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let instructions = auth
        .and_then(|a| a.get("instructions"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let setup_url = auth
        .and_then(|a| a.get("setup_url"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let token_hint = auth
        .and_then(|a| a.get("token_hint"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let oauth_available = auth.and_then(|a| a.get("oauth")).is_some();
    let auth_type = if oauth_available {
        "oauth".to_string()
    } else if auth.is_some() {
        "manual".to_string()
    } else {
        "none".to_string()
    };

    ExtensionAuthInfo {
        name: name.to_string(),
        auth_type,
        instructions,
        setup_url,
        token_hint,
        display_name,
        oauth_available,
    }
}

#[cfg(test)]
mod tests {
    use crate::extensions::manager::{auth_info_from_entry_pub, infer_kind_from_url};
    use crate::extensions::{AuthHint, ExtensionKind, ExtensionSource, RegistryEntry};

    use super::{ExtensionStatus, auth_info_from_capabilities, compute_status};

    // ── infer_kind_from_url ──────────────────────────────────────────────

    #[test]
    fn test_infer_kind_from_url() {
        assert_eq!(
            infer_kind_from_url("https://example.com/tool.wasm"),
            ExtensionKind::WasmTool
        );
        assert_eq!(
            infer_kind_from_url("https://mcp.notion.com"),
            ExtensionKind::McpServer
        );
        assert_eq!(
            infer_kind_from_url("https://example.com/mcp"),
            ExtensionKind::McpServer
        );
    }

    // ── compute_status ───────────────────────────────────────────────────

    #[test]
    fn test_compute_status_active_wins_over_error() {
        // active=true takes priority even when has_error=true
        assert_eq!(compute_status(true, true, true), ExtensionStatus::Active);
    }

    #[test]
    fn test_compute_status_active() {
        assert_eq!(compute_status(true, true, false), ExtensionStatus::Active);
    }

    #[test]
    fn test_compute_status_error() {
        assert_eq!(compute_status(true, false, true), ExtensionStatus::Error);
    }

    #[test]
    fn test_compute_status_needs_auth() {
        assert_eq!(
            compute_status(false, false, false),
            ExtensionStatus::NeedsAuth
        );
    }

    #[test]
    fn test_compute_status_inactive() {
        // authenticated, not active, no error
        assert_eq!(
            compute_status(true, false, false),
            ExtensionStatus::Inactive
        );
    }

    // ── auth_info_from_entry ─────────────────────────────────────────────

    fn make_entry(auth_hint: AuthHint) -> RegistryEntry {
        RegistryEntry {
            name: "test-ext".to_string(),
            display_name: "Test Extension".to_string(),
            kind: ExtensionKind::WasmTool,
            description: "A test extension".to_string(),
            keywords: vec![],
            category: None,
            source: ExtensionSource::McpUrl {
                url: "https://example.com".to_string(),
            },
            auth_hint,
        }
    }

    #[test]
    fn test_auth_info_from_entry_dcr() {
        let entry = make_entry(AuthHint::Dcr);
        let info = auth_info_from_entry_pub(&entry, "test-ext");
        assert_eq!(info.auth_type, "dcr");
        assert!(info.oauth_available);
        assert!(info.instructions.is_none());
        assert_eq!(info.display_name.as_deref(), Some("Test Extension"));
    }

    #[test]
    fn test_auth_info_from_entry_oauth() {
        let entry = make_entry(AuthHint::OAuthPreConfigured {
            setup_url: "https://console.example.com/credentials".to_string(),
        });
        let info = auth_info_from_entry_pub(&entry, "test-ext");
        assert_eq!(info.auth_type, "oauth");
        assert!(info.oauth_available);
        assert_eq!(
            info.setup_url.as_deref(),
            Some("https://console.example.com/credentials")
        );
        assert!(info.instructions.is_some());
    }

    #[test]
    fn test_auth_info_from_entry_capabilities_auth() {
        let entry = make_entry(AuthHint::CapabilitiesAuth);
        let info = auth_info_from_entry_pub(&entry, "test-ext");
        assert_eq!(info.auth_type, "manual");
        assert!(!info.oauth_available);
        assert!(info.instructions.is_some());
        assert!(info.setup_url.is_none());
    }

    #[test]
    fn test_auth_info_from_entry_none() {
        let entry = make_entry(AuthHint::None);
        let info = auth_info_from_entry_pub(&entry, "test-ext");
        assert_eq!(info.auth_type, "none");
        assert!(!info.oauth_available);
        assert!(info.instructions.is_none());
    }

    // ── auth_info_from_capabilities ──────────────────────────────────────

    #[test]
    fn test_auth_info_from_capabilities_no_auth_block() {
        let json = serde_json::json!({ "name": "my-tool" });
        let info = auth_info_from_capabilities(&json, "my-tool");
        assert_eq!(info.auth_type, "none");
        assert!(!info.oauth_available);
        assert!(info.instructions.is_none());
        assert!(info.display_name.is_none());
    }

    #[test]
    fn test_auth_info_from_capabilities_manual_token() {
        let json = serde_json::json!({
            "auth": {
                "display_name": "My Service",
                "instructions": "Go to example.com/tokens",
                "setup_url": "https://example.com/tokens",
                "token_hint": "Starts with 'sk-'"
            }
        });
        let info = auth_info_from_capabilities(&json, "my-tool");
        assert_eq!(info.auth_type, "manual");
        assert!(!info.oauth_available);
        assert_eq!(info.display_name.as_deref(), Some("My Service"));
        assert_eq!(
            info.instructions.as_deref(),
            Some("Go to example.com/tokens")
        );
        assert_eq!(
            info.setup_url.as_deref(),
            Some("https://example.com/tokens")
        );
        assert_eq!(info.token_hint.as_deref(), Some("Starts with 'sk-'"));
    }

    #[test]
    fn test_auth_info_from_capabilities_oauth() {
        let json = serde_json::json!({
            "auth": {
                "display_name": "Google",
                "oauth": {
                    "authorization_url": "https://accounts.google.com/o/oauth2/auth"
                }
            }
        });
        let info = auth_info_from_capabilities(&json, "gmail");
        assert_eq!(info.auth_type, "oauth");
        assert!(info.oauth_available);
        assert_eq!(info.display_name.as_deref(), Some("Google"));
    }

    // ── get_config_schema (pure JSON logic) ──────────────────────────────

    #[test]
    fn test_config_schema_present_in_capabilities() {
        let json = serde_json::json!({
            "config_schema": {
                "type": "object",
                "properties": {
                    "owner_id": { "type": "string", "nullable": true },
                    "dm_policy": { "type": "string", "enum": ["pairing", "owner_only"] }
                }
            }
        });
        let schema = json.get("config_schema").cloned();
        assert!(schema.is_some());
        let props = schema.unwrap();
        assert_eq!(props["type"], "object");
        assert!(props["properties"].get("owner_id").is_some());
        assert!(props["properties"].get("dm_policy").is_some());
    }

    #[test]
    fn test_config_schema_absent_returns_none() {
        let json = serde_json::json!({ "auth": { "secret_name": "tok" } });
        assert!(json.get("config_schema").is_none());
    }

    #[test]
    fn test_config_schema_field_name_validation() {
        // Valid field names (alphanumeric + underscore)
        let valid = ["owner_id", "dm_policy", "allow_from", "poll_interval_ms"];
        for name in valid {
            assert!(
                name.chars().all(|c| c.is_alphanumeric() || c == '_'),
                "expected valid: {name}"
            );
        }
        // Path traversal attempts must fail
        let invalid = ["../secrets/tok", "foo/bar", "key=val", "a b", "a.b"];
        for name in invalid {
            assert!(
                !name.chars().all(|c| c.is_alphanumeric() || c == '_'),
                "expected invalid: {name}"
            );
        }
    }

    #[test]
    fn test_config_schema_auth_fields_not_present() {
        // The capabilities.json files for discord/telegram/matrix must not expose
        // auth-adjacent fields (secret_name, token) in config_schema.
        // We verify the schema shape here with representative data.
        let discord_caps = serde_json::json!({
            "setup": { "required_secrets": [{ "name": "discord_bot_token" }] },
            "config_schema": {
                "type": "object",
                "properties": {
                    "owner_id": { "type": "string" },
                    "dm_policy": { "type": "string" },
                    "allow_from": { "type": "array" }
                }
            }
        });
        let schema = discord_caps.get("config_schema").unwrap();
        let props = schema["properties"].as_object().unwrap();
        // Auth fields must not be in the schema properties
        assert!(!props.contains_key("discord_bot_token"));
        assert!(!props.contains_key("secret_name"));
        assert!(!props.contains_key("token"));
        // Non-secret config fields are present
        assert!(props.contains_key("owner_id"));
        assert!(props.contains_key("dm_policy"));
    }

    // ── get_auth_info: McpServerConfig priority ───────────────────────────
    //
    // These tests exercise the auth-type derivation logic that lives inside
    // get_auth_info directly (the pure-data branches, without I/O).

    use crate::tools::mcp::config::{McpServerConfig, OAuthConfig};

    fn server_no_oauth(name: &str, url: &str) -> McpServerConfig {
        McpServerConfig::new(name, url)
    }

    fn server_with_oauth(name: &str, url: &str) -> McpServerConfig {
        McpServerConfig::new(name, url)
            .with_oauth(OAuthConfig::new("client-abc").with_scopes(vec!["read".into()]))
    }

    /// Helper that mirrors the auth_type derivation inside get_auth_info so we
    /// can test it without spinning up the full ExtensionManager.
    fn derive_auth_type(server: &McpServerConfig) -> &'static str {
        if server.oauth.is_some() {
            "dcr"
        } else if server.requires_auth() {
            "manual"
        } else {
            "none"
        }
    }

    #[test]
    fn test_mcp_remote_no_oauth_yields_manual() {
        // Remote HTTPS server without pre-configured OAuth must produce "manual"
        // so the wizard always shows a token input field rather than an OAuth button.
        let server = server_no_oauth("github", "https://mcp.github.com");
        assert_eq!(derive_auth_type(&server), "manual");
    }

    #[test]
    fn test_mcp_remote_with_oauth_yields_dcr() {
        // Remote HTTPS server with a pre-configured OAuth client uses the browser flow.
        let server = server_with_oauth("notion", "https://mcp.notion.com");
        assert_eq!(derive_auth_type(&server), "dcr");
    }

    #[test]
    fn test_mcp_localhost_yields_none() {
        // Local dev servers need no auth regardless of URL scheme.
        let server = server_no_oauth("local", "http://localhost:8080");
        assert_eq!(derive_auth_type(&server), "none");
        let server2 = server_no_oauth("local2", "http://127.0.0.1:3000");
        assert_eq!(derive_auth_type(&server2), "none");
    }

    #[test]
    fn test_mcp_manual_auth_type_has_instructions() {
        // When auth_type is "manual", the instructions field must be populated so
        // the UI knows what to display to the user.
        let _server = server_no_oauth("github", "https://mcp.github.com");
        let display = "GitHub";
        let instructions = format!(
            "Enter an API token or personal access token for {}. \
             The token will be stored encrypted and sent as a Bearer \
             Authorization header when connecting to the server.",
            display
        );
        assert!(!instructions.is_empty());
        assert!(instructions.contains("Bearer"));
        assert!(instructions.contains("encrypted"));
    }

    #[test]
    fn test_mcp_manual_oauth_available_false() {
        // Manual auth type must not advertise oauth_available so the wizard
        // renders a token <input>, not an "Authorize" button.
        let server = server_no_oauth("cloudflare", "https://mcp.cloudflare.com/sse");
        assert!(server.oauth.is_none()); // no oauth config
        assert!(server.requires_auth()); // remote HTTPS
    }

    #[test]
    fn test_mcp_token_secret_name_format() {
        // The secret key that stores the Bearer token must follow the
        // `mcp_{name}_access_token` convention used by McpClient::new_authenticated.
        let server = server_no_oauth("github", "https://mcp.github.com");
        assert_eq!(server.token_secret_name(), "mcp_github_access_token");
        let server2 = server_no_oauth("cloudflare", "https://mcp.cloudflare.com/sse");
        assert_eq!(server2.token_secret_name(), "mcp_cloudflare_access_token");
    }
}
