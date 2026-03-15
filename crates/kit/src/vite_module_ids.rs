use camino::Utf8PathBuf;

use crate::filesystem::posixify;

pub const fn env_static_private_module_id() -> &'static str {
    "\0virtual:env/static/private"
}

pub const fn env_static_public_module_id() -> &'static str {
    "\0virtual:env/static/public"
}

pub const fn env_dynamic_private_module_id() -> &'static str {
    "\0virtual:env/dynamic/private"
}

pub const fn env_dynamic_public_module_id() -> &'static str {
    "\0virtual:env/dynamic/public"
}

pub const fn service_worker_module_id() -> &'static str {
    "\0virtual:service-worker"
}

pub const fn sveltekit_environment_module_id() -> &'static str {
    "\0virtual:__sveltekit/environment"
}

pub const fn sveltekit_server_module_id() -> &'static str {
    "\0virtual:__sveltekit/server"
}

pub fn app_server_module_id(src_root: Utf8PathBuf) -> String {
    posixify(src_root.join("runtime/app/server/index.js").as_str())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViteModuleIds {
    pub env_static_private: &'static str,
    pub env_static_public: &'static str,
    pub env_dynamic_private: &'static str,
    pub env_dynamic_public: &'static str,
    pub service_worker: &'static str,
    pub sveltekit_environment: &'static str,
    pub sveltekit_server: &'static str,
    pub app_server: String,
}

impl ViteModuleIds {
    pub fn for_src_root(src_root: Utf8PathBuf) -> Self {
        Self {
            env_static_private: env_static_private_module_id(),
            env_static_public: env_static_public_module_id(),
            env_dynamic_private: env_dynamic_private_module_id(),
            env_dynamic_public: env_dynamic_public_module_id(),
            service_worker: service_worker_module_id(),
            sveltekit_environment: sveltekit_environment_module_id(),
            sveltekit_server: sveltekit_server_module_id(),
            app_server: app_server_module_id(src_root),
        }
    }
}
