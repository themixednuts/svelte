use camino::Utf8PathBuf;
use svelte_kit::{
    ViteModuleIds, app_server_module_id, env_dynamic_private_module_id,
    env_dynamic_public_module_id, env_static_private_module_id, env_static_public_module_id,
    service_worker_module_id, sveltekit_environment_module_id, sveltekit_server_module_id,
};

#[test]
fn vite_module_ids_match_upstream_virtual_ids() {
    assert_eq!(
        env_static_private_module_id(),
        "\0virtual:env/static/private"
    );
    assert_eq!(env_static_public_module_id(), "\0virtual:env/static/public");
    assert_eq!(
        env_dynamic_private_module_id(),
        "\0virtual:env/dynamic/private"
    );
    assert_eq!(
        env_dynamic_public_module_id(),
        "\0virtual:env/dynamic/public"
    );
    assert_eq!(service_worker_module_id(), "\0virtual:service-worker");
    assert_eq!(
        sveltekit_environment_module_id(),
        "\0virtual:__sveltekit/environment"
    );
    assert_eq!(sveltekit_server_module_id(), "\0virtual:__sveltekit/server");
}

#[test]
fn app_server_module_id_is_posix_path() {
    let module_id =
        app_server_module_id(Utf8PathBuf::from("E:/Projects/svelte/kit/packages/kit/src"));
    assert!(module_id.ends_with("/runtime/app/server/index.js"));
    assert!(!module_id.contains('\\'));
}

#[test]
fn convenience_struct_exposes_all_module_ids() {
    let ids =
        ViteModuleIds::for_src_root(Utf8PathBuf::from("E:/Projects/svelte/kit/packages/kit/src"));
    assert_eq!(ids.env_static_private, env_static_private_module_id());
    assert_eq!(ids.sveltekit_server, sveltekit_server_module_id());
    assert!(ids.app_server.ends_with("/runtime/app/server/index.js"));
}
