use crate::Result;
use clap::Parser;
use nostr_relay::App;
use nostr_relay::Extension;
use std::path::PathBuf;
use tracing::{info, warn};

/// Start relay options
#[derive(Debug, Clone, Parser)]
pub struct RelayOpts {
    /// Nostr relay config path
    #[arg(
        short = 'c',
        value_name = "PATH",
        default_value = "./config/rnostr.toml"
    )]
    pub config: PathBuf,

    /// Auto reload when config changed
    #[arg(long, value_name = "BOOL")]
    pub watch: bool,
}

#[actix_rt::main]
pub async fn relay(config: &PathBuf, watch: bool) -> Result<()> {
    tracing_subscriber::fmt::init();
    info!("Start relay server");

    // actix_rt::System::new().block_on(async {
    // });

    let app_data = App::create(Some(config), watch, Some("RNOSTR".to_owned()), None)?;
    let db = app_data.db.clone();

    // Startup Firestore -> LMDB backfill if configured (no REST dependency)
    {
        let r = app_data.setting.read();
        let mgcfg: nostr_extensions::mls_gateway::MlsGatewayConfig = r.parse_extension("mls_gateway");
        drop(r);

        if mgcfg.backfill_on_startup {
            info!(
                "Startup backfill enabled: kinds={:?}, max_events={}",
                mgcfg.backfill_kinds, mgcfg.backfill_max_events
            );
            match nostr_extensions::mls_gateway::MessageArchive::new().await {
                Ok(archive) => {
                    let since = chrono::Utc::now().timestamp()
                        - (mgcfg.message_archive_ttl_days as i64) * 86_400;
                    match archive
                        .list_recent_events_by_kinds(
                            &mgcfg.backfill_kinds,
                            since,
                            mgcfg.backfill_max_events,
                        )
                        .await
                    {
                        Ok(events) => {
                            if !events.is_empty() {
                                match db.batch_put(events) {
                                    Ok(count) => info!("Backfilled {} events into LMDB", count),
                                    Err(e) => warn!("Backfill batch_put error: {}", e),
                                }
                            } else {
                                info!(
                                    "No events to backfill from Firestore (within TTL window)"
                                );
                            }
                        }
                        Err(e) => warn!("Backfill query failed: {}", e),
                    }
                }
                Err(e) => warn!("MessageArchive init failed; skipping backfill: {}", e),
            }
        } else {
            info!("Startup backfill disabled by configuration");
        }
    }
    // Initialize MLS Gateway with loaded settings before adding the extension
    let mut mls_gateway = nostr_extensions::MlsGateway::new(Default::default());
    // Apply current settings from App so the gateway picks up config (e.g., Firestore project_id)
    mls_gateway.setting(&app_data.setting);
    if let Err(e) = mls_gateway.initialize().await {
        warn!("MLS Gateway initialization failed: {}", e);
    }

    app_data
        .add_extension(nostr_extensions::Metrics::new())
        .add_extension(nostr_extensions::Auth::new())
        .add_extension(nostr_extensions::Ratelimiter::new())
        .add_extension(nostr_extensions::Count::new(db))
        .add_extension(nostr_extensions::Search::new())
        .add_extension(mls_gateway)
        .add_extension(nostr_extensions::NipService::new())
        .web_server()?
        .await?;
    info!("Relay server shutdown");

    Ok(())
}
