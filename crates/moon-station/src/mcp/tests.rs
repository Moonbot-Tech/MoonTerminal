//! `[mcp]` configuration: the public origin, the tunnel ID, and which of them a station needs.

use super::*;

/// Either way in is enough on its own; a tunnel-only section has no public origin to check.
#[test]
fn a_section_names_the_public_origin_the_tunnel_or_both() {
    let id = "tunnel_0123456789abcdef0123456789abcdef";
    let both: Config = toml::from_str(&format!(
        "public_url = 'https://203.0.113.7'\ntunnel_id = '{id}'"
    ))
    .unwrap();
    assert_eq!(
        both.origin().unwrap().as_deref(),
        Some("https://203.0.113.7")
    );
    assert_eq!(both.tunnel().unwrap().as_deref(), Some(id));
    let tunnel: Config = toml::from_str(&format!("tunnel_id = '{id}'")).unwrap();
    assert_eq!(tunnel.origin().unwrap(), None);
    let public: Config = toml::from_str("public_url = 'https://203.0.113.7'").unwrap();
    assert_eq!(public.tunnel().unwrap(), None);
    assert!(
        toml::from_str::<Config>("tunel_id = 'x'").is_err(),
        "a typo is refused"
    );
}

/// The ID goes into a URL path on OpenAI's API: only OpenAI's own shapes pass.
#[test]
fn only_openai_tunnel_ids_are_accepted() {
    assert!(valid_tunnel_id("tunnel_0123456789abcdef0123456789abcdef"));
    assert!(valid_tunnel_id(
        "tunnel_ab12_0123456789abcdef0123456789abcdef"
    ));
    for bad in [
        "",
        "tunnel_",
        "tunnel_0123456789ABCDEF0123456789ABCDEF",
        "tunnel_0123456789abcdef0123456789abcde",
        "tunnel_0123456789abcdef0123456789abcdef/../x",
        "tunnel_abc_0123456789abcdef0123456789abcdef",
        "0123456789abcdef0123456789abcdef",
    ] {
        assert!(!valid_tunnel_id(bad), "{bad}");
    }
}
