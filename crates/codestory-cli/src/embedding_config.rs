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
