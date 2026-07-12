const BUNDLED_LLAMACPP_BACKEND_LABEL: &str = "llamacpp";

pub(crate) fn prepare_bundled_llamacpp_client_env_defaults() {
    unsafe {
        if env_value_is_unset("CODESTORY_EMBED_RUNTIME_MODE")
            && env_value_is_unset("CODESTORY_EMBED_BACKEND")
        {
            set_env_default_str("CODESTORY_EMBED_BACKEND", BUNDLED_LLAMACPP_BACKEND_LABEL);
        }
    }
}

fn env_value_is_unset(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
}

unsafe fn set_env_default_str(key: &str, value: &str) {
    if std::env::var_os(key).is_none() {
        unsafe {
            std::env::set_var(key, value);
        }
    }
}

pub(crate) fn redact_url_for_display(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    let rest = rest
        .split_once('#')
        .map(|(before, _)| before)
        .unwrap_or(rest);
    let rest = rest
        .split_once('?')
        .map(|(before, _)| before)
        .unwrap_or(rest);
    let (authority, suffix) = rest.split_once('/').unwrap_or((rest, ""));
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    if suffix.is_empty() {
        format!("{scheme}://{host_port}")
    } else {
        format!("{scheme}://{host_port}/{suffix}")
    }
}
