//! Exercise the native startup loaders against a fresh, isolated configuration.
//! No native app, Keychain, authentication prompt or network connection is started.
use crate::{admin, demo::DemoCluster, inventory::Inventory, llm::ProviderSettings, prefs::{DataSource, Prefs}, server::ServerSettings, util, vpn::VpnSettings};

#[test]
fn fresh_distribution_has_no_connections_and_keeps_existing_configuration() {
    let dir = crate::fake_node::temp("distribution-startup");
    let inventory_path = dir.join("inventory.json");
    let inv = Inventory::load(&inventory_path);
    assert!(inv.nodes.is_empty() && inv.switches.is_empty());
    let snapshot = DemoCluster::new(&inv).snapshot();
    assert!(snapshot.demo && snapshot.nodes.is_empty() && snapshot.switches.is_empty());
    let prefs = Prefs::load(&dir.join("prefs.json"));
    assert_eq!(prefs.data_source, DataSource::Demo);
    assert!(!prefs.hardware_auto_lookup);
    let vpn: VpnSettings = util::load_json_or_default(&dir.join("vpn.json"));
    assert!(vpn.profile.is_none() && vpn.forwards.is_empty());
    let providers: ProviderSettings = util::load_json_or_default(&dir.join("providers.json"));
    assert!(providers.discovered_prefs.is_empty());
    assert_eq!(providers.providers.iter().map(|p| p.base_url.as_str()).collect::<Vec<_>>(),
        ["https://api.anthropic.com", "https://api.openai.com/v1", ""]);
    let server: ServerSettings = util::load_json_or_default(&dir.join("server.json"));
    assert!(!server.autostart && server.require_key);
    assert!(server.aliases.is_empty() && server.allowed_origins.is_empty());
    let auth = admin::testing::FakeAuth::default();
    let gate = admin::AdminGate::new(Box::new(admin::testing::FakeClock::default()), Box::new(auth.clone()), Some(dir.join("security.json")));
    assert!(!gate.state().unlocked);
    assert_eq!(auth.asked.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Empty state stays empty after saving/reopening; fixture data is opt-in only.
    inv.save(&inventory_path).unwrap();
    assert_eq!(Inventory::load(&inventory_path), inv);
    let saved = Inventory::example();
    saved.save(&inventory_path).unwrap();
    assert_eq!(Inventory::load(&inventory_path), saved, "an upgrade must not reset an existing user's nodes");
    std::fs::remove_dir_all(dir).unwrap();
}
