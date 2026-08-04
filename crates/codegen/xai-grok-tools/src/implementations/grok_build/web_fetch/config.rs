//! Runtime-configurable parameters for the `web_fetch` tool.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::register_resource;

// Safety-boundary constants. Not configurable.
pub const MAX_URL_LENGTH: usize = 2_000;
pub const MAX_REDIRECTS: usize = 10;
/// Browser-shaped default UA that still identifies Turbo Grok as a research agent.
/// Configurable via `[toolset.web_fetch] user_agent`.
pub const USER_AGENT_STRING: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 TurboGrok/1.0 (research agent; +https://x.ai)";

/// Browser-like Accept: HTML first, markdown still preferred when available.
pub const ACCEPT_HEADER: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,\
text/markdown;q=0.9,image/avif,image/webp,*/*;q=0.8";

/// Runtime-configurable parameters for the `web_fetch` tool.
///
/// Injected via `Params<WebFetchParams>` in `SharedResources`.
/// All fields are optional — `None` means "use built-in default."
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebFetchParams {
    /// Cache time-to-live in seconds. Default: 900 (15 minutes).
    pub cache_ttl_secs: Option<u64>,
    /// Maximum number of cached pages. Default: 128.
    pub max_cache_entries: Option<usize>,
    /// HTTP request timeout in seconds. Default: 60.
    pub timeout_secs: Option<u64>,
    /// Maximum response body size in bytes. Default: 10 MB.
    pub max_content_length: Option<usize>,
    /// Maximum inline markdown output length in bytes. Default: 100,000.
    pub max_markdown_length: Option<usize>,
    /// Model context window size in tokens. Used to enforce 3% cap on web content.
    pub context_window_tokens: Option<u64>,
    /// Optional domain allowlist. When `None` (default), any public host
    /// that passes SSRF may be fetched (open-public policy). When `Some(list)`,
    /// only listed hosts (and optional path prefixes like `vercel.com/docs`)
    /// are allowed. An explicit empty list is treated as "block all" by the
    /// shell (tool disabled). See `DEFAULT_ALLOWED_DOMAINS` for a curated
    /// preset enterprises can copy into config.
    #[serde(default)]
    pub allowed_domains: Option<Vec<String>>,
    /// Optional egress proxy endpoint. When set, all HTTP requests are
    /// routed through this URL.
    #[serde(default)]
    pub proxy_endpoint: Option<String>,
    /// When true, allow fetches to **explicit** loopback hosts only
    /// (`localhost`, `127.0.0.0/8`, `::1`). Private/metadata stay blocked.
    /// Default: `false` (fail closed). Set via `[toolset.web_fetch]
    /// allow_local = true` or `GROK_WEB_FETCH_ALLOW_LOCAL=1`.
    #[serde(default)]
    pub allow_local: Option<bool>,
    /// `Accept-Language` header value. Default: `en-US,en;q=0.9`.
    #[serde(default)]
    pub accept_language: Option<String>,
    /// Maximum JavaScript response size in **bytes** before soft truncate.
    /// Default: 256 KiB. Truncation is applied after decode (char-aware).
    #[serde(default)]
    pub max_js_bytes: Option<usize>,
    /// Override User-Agent. Default: browser-shaped TurboGrok research UA.
    #[serde(default)]
    pub user_agent: Option<String>,
}

register_resource!("grok_build", "WebFetch", WebFetchParams);

// Keep defaults here so call-sites don't have to manage unwrapping.
// Vars are still public following other conventions though.
impl WebFetchParams {
    pub fn cache_ttl_secs(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_secs.unwrap_or(15 * 60))
    }

    pub fn max_cache_entries(&self) -> usize {
        self.max_cache_entries.unwrap_or(128)
    }

    pub fn timeout_secs(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.unwrap_or(60))
    }

    pub fn max_content_length(&self) -> usize {
        self.max_content_length.unwrap_or(10 * 1024 * 1024)
    }

    pub fn max_markdown_length(&self) -> usize {
        self.max_markdown_length.unwrap_or(100_000)
    }

    pub fn context_window_tokens(&self) -> u64 {
        self.context_window_tokens.unwrap_or(128_000)
    }

    pub fn allow_local(&self) -> bool {
        self.allow_local.unwrap_or(false)
    }

    pub fn accept_language(&self) -> &str {
        self.accept_language
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("en-US,en;q=0.9")
    }

    pub fn max_js_bytes(&self) -> usize {
        self.max_js_bytes.unwrap_or(256 * 1024)
    }

    pub fn user_agent(&self) -> &str {
        self.user_agent
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(USER_AGENT_STRING)
    }

    /// Returns `Some(list)` when an allowlist is configured, or `None` for
    /// open-public mode (SSRF still applies).
    pub fn domain_allowlist(&self) -> Option<&[String]> {
        self.allowed_domains.as_deref()
    }

    /// Materialised allowlist for callers that need a concrete list.
    ///
    /// - `Some(list)` → that list  
    /// - `None` (open-public) → **empty** vec (no static grants)  
    ///
    /// Prefer [`Self::domain_allowlist`] when you need to distinguish open-public
    /// (`None`) from an explicit empty block-all list (`Some([])`).
    ///
    /// Note: this no longer expands `None` to [`DEFAULT_ALLOWED_DOMAINS`].
    pub fn allowed_domains(&self) -> Vec<String> {
        self.allowed_domains.clone().unwrap_or_default()
    }
}

/// Curated docs-oriented domain preset for enterprises that want a tight
/// allowlist. Not applied by default — open-public is the product default
/// (SSRF still blocks private/metadata). Copy into
/// `[toolset.web_fetch] allowed_domains = [...]` to enforce.
///
/// Note: GET-only preapproved domains. Path-scoped entries (e.g. vercel.com/docs) are included as-is.
pub static DEFAULT_ALLOWED_DOMAINS: &[&str] = &[
    // xAI
    "x.ai",
    "console.x.ai",
    "docs.x.ai",
    "api.x.ai",
    // Programming languages
    "docs.python.org",
    "en.cppreference.com",
    "docs.oracle.com",
    "learn.microsoft.com",
    "developer.mozilla.org",
    "go.dev",
    "pkg.go.dev",
    "www.php.net",
    "docs.swift.org",
    "kotlinlang.org",
    "ruby-doc.org",
    "doc.rust-lang.org",
    "docs.rs",
    "www.typescriptlang.org",
    // Web and JS frameworks
    "react.dev",
    "angular.io",
    "vuejs.org",
    "nextjs.org",
    "expressjs.com",
    "nodejs.org",
    "bun.sh",
    "jquery.com",
    "getbootstrap.com",
    "tailwindcss.com",
    "d3js.org",
    "threejs.org",
    "redux.js.org",
    "webpack.js.org",
    "jestjs.io",
    "reactrouter.com",
    // Python frameworks
    "docs.djangoproject.com",
    "flask.palletsprojects.com",
    "fastapi.tiangolo.com",
    "pandas.pydata.org",
    "numpy.org",
    "www.tensorflow.org",
    "pytorch.org",
    "scikit-learn.org",
    "matplotlib.org",
    "requests.readthedocs.io",
    "jupyter.org",
    // PHP frameworks
    "laravel.com",
    "symfony.com",
    "wordpress.org",
    // Java frameworks
    "docs.spring.io",
    "hibernate.org",
    "tomcat.apache.org",
    "gradle.org",
    "maven.apache.org",
    // .NET
    "asp.net",
    "dotnet.microsoft.com",
    "nuget.org",
    "blazor.net",
    // Mobile
    "reactnative.dev",
    "docs.flutter.dev",
    "developer.apple.com",
    "developer.android.com",
    // Data science / ML
    "keras.io",
    "spark.apache.org",
    "huggingface.co",
    "www.kaggle.com",
    // Databases
    "redis.io",
    "www.postgresql.org",
    "dev.mysql.com",
    "www.sqlite.org",
    "graphql.org",
    "prisma.io",
    // Cloud and DevOps
    "docs.aws.amazon.com",
    "cloud.google.com",
    "kubernetes.io",
    "www.docker.com",
    "www.terraform.io",
    "www.ansible.com",
    "vercel.com/docs",
    "docs.netlify.com",
    "devcenter.heroku.com",
    // Testing and monitoring
    "cypress.io",
    "selenium.dev",
    // Game development
    "docs.unity.com",
    "docs.unrealengine.com",
    // HYPER-LOCAL: Godot + Blender reference. GDScript and bpy are both
    // under-represented in training data relative to the languages they
    // resemble (Python), so doc lookups are frequent and inventing an API that
    // does not exist is the dominant failure mode. Allowlisting these removes a
    // permission prompt from every lookup.
    "docs.godotengine.org",
    "docs.blender.org",
    "developer.blender.org",
    // Other tools
    "git-scm.com",
    "nginx.org",
    "httpd.apache.org",
];
