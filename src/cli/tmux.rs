//! `aoe tmux` command implementation

use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum TmuxCommands {
    /// Output session info for use in custom tmux status bar
    ///
    /// Add this to your ~/.tmux.conf:
    ///   set -g status-right "#(aoe tmux status)"
    Status(TmuxStatusArgs),

    /// Output peer session statuses for tmux status bar
    ///
    /// Shows up to 5 most recently active peer sessions with status icons.
    /// Designed to be embedded in tmux status-right via #(aoe tmux peers).
    Peers,

    /// Switch to the Nth peer session (1-indexed)
    ///
    /// Used by tmux keybindings (Ctrl+B 1 through Ctrl+B 5) to quickly jump
    /// to another aoe session.
    Switch(TmuxSwitchArgs),
}

#[derive(Args)]
pub struct TmuxStatusArgs {
    /// Output format (text or json)
    #[arg(short, long, default_value = "text")]
    format: String,
}

#[derive(Args)]
pub struct TmuxSwitchArgs {
    /// Session index (1-5)
    pub index: usize,
}

pub fn run_status(args: TmuxStatusArgs) -> Result<()> {
    use crate::tmux::status_bar::get_session_info_for_current;

    match get_session_info_for_current() {
        Some(info) => {
            if args.format == "json" {
                let json = serde_json::json!({
                    "title": info.title,
                    "branch": info.branch,
                    "sandbox": info.sandbox,
                });
                println!("{}", serde_json::to_string(&json)?);
            } else {
                let mut output = format!("aoe: {}", info.title);
                if let Some(b) = &info.branch {
                    output.push_str(" | ");
                    output.push_str(b);
                }
                if let Some(s) = &info.sandbox {
                    output.push_str(" [");
                    output.push_str(s);
                    output.push(']');
                }
                print!("{}", output);
            }
        }
        None => {
            // Not in an aoe session - output nothing (cleaner for tmux status bar)
            if args.format == "json" {
                println!("null");
            }
        }
    }

    Ok(())
}

struct PeerSession {
    title: String,
    icon: &'static str,
    tmux_name: String,
    session_id: String,
    profile: String,
    group_prefix: Option<String>,
}

/// Get list of peer sessions (other aoe sessions, excluding current).
/// Sorted by last_user_activity descending, limited to 5.
fn get_peer_sessions() -> Vec<PeerSession> {
    use crate::hooks::read_hook_status;
    use crate::session::{list_profiles, GroupTree, Status, Storage};
    use crate::tmux;

    let current_tmux = tmux::get_current_session_name().unwrap_or_default();

    let profiles = list_profiles().unwrap_or_else(|_| vec!["default".to_string()]);
    let mut peers: Vec<(PeerSession, Option<chrono::DateTime<chrono::Utc>>)> = Vec::new();

    for profile in &profiles {
        let storage = match Storage::new(profile) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (instances, groups) = match storage.load_with_groups() {
            Ok(r) => r,
            Err(_) => continue,
        };
        let tree = GroupTree::new_with_groups(&instances, &groups);

        for inst in &instances {
            let tmux_name = tmux::Session::generate_name(&inst.id, &inst.title);

            if tmux_name == current_tmux {
                continue;
            }

            let session = tmux::Session::new(&inst.id, &inst.title);
            let exists = session.as_ref().map(|s| s.exists()).unwrap_or(false);
            if !exists {
                continue;
            }

            let status = read_hook_status(&inst.id).unwrap_or(Status::Idle);
            let icon = match status {
                Status::Running => "\u{25cf}",
                Status::Waiting => "\u{25d0}",
                _ => "\u{25cb}",
            };

            let group_prefix = if inst.group_path.is_empty() {
                None
            } else {
                Some(
                    tree.get_group(&inst.group_path)
                        .map(|g| g.display_prefix())
                        .unwrap_or_else(|| {
                            inst.group_path
                                .rsplit('/')
                                .next()
                                .unwrap_or(&inst.group_path)
                                .chars()
                                .take(3)
                                .collect::<String>()
                                .to_uppercase()
                        }),
                )
            };

            peers.push((
                PeerSession {
                    title: inst.title.clone(),
                    icon,
                    tmux_name,
                    session_id: inst.id.clone(),
                    profile: profile.clone(),
                    group_prefix,
                },
                inst.last_user_activity,
            ));
        }
    }

    // Sort by last user activity (most recent first)
    peers.sort_by(|a, b| {
        let a_time = a.1.unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC);
        let b_time = b.1.unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC);
        b_time.cmp(&a_time)
    });

    peers.into_iter().take(5).map(|(p, _)| p).collect()
}

/// Format peers into a tmux-styled string.
fn format_peers_string(peers: &[PeerSession]) -> String {
    use crate::session::config::Config;
    use crate::tui::styles::load_theme;

    if peers.is_empty() {
        return String::new();
    }

    let config = Config::load().unwrap_or_default();
    let theme_name = if config.theme.name.is_empty() {
        "empire"
    } else {
        &config.theme.name
    };
    let theme = load_theme(theme_name);

    let running_hex = crate::tmux::status_bar::color_to_tmux_pub(theme.running);
    let waiting_hex = crate::tmux::status_bar::color_to_tmux_pub(theme.waiting);
    let idle_hex = crate::tmux::status_bar::color_to_tmux_pub(theme.idle);
    let hint_hex = crate::tmux::status_bar::color_to_tmux_pub(theme.dimmed);

    let mut parts = Vec::new();
    for (i, peer) in peers.iter().enumerate() {
        let short_title: String = peer.title.chars().take(20).collect();
        let color = match peer.icon {
            "\u{25cf}" => &running_hex,
            "\u{25d0}" => &waiting_hex,
            _ => &idle_hex,
        };
        let display = match &peer.group_prefix {
            Some(pre) => format!("[{}] {}", pre, short_title),
            None => short_title,
        };
        parts.push(format!(
            "#[fg={hint_hex}]{}#[fg={color}]{} {}",
            i + 1,
            peer.icon,
            display,
        ));
    }

    parts.join(&format!(" #[fg={hint_hex}]\u{2502} "))
}

/// Set @aoe_peers option on a specific tmux session.
fn set_peers_on_session(session_name: &str, peers_str: &str) {
    let _ = std::process::Command::new("tmux")
        .args(["set-option", "-t", session_name, "@aoe_peers", peers_str])
        .output();
}

pub fn run_peers() -> Result<()> {
    let peers = get_peer_sessions();
    let formatted = format_peers_string(&peers);

    // Set @aoe_peers on the current session for instant display
    if let Some(current) = crate::tmux::get_current_session_name() {
        set_peers_on_session(&current, &formatted);
    }

    // Print nothing - output goes to @aoe_peers variable, not stdout
    Ok(())
}

pub fn run_switch(args: TmuxSwitchArgs) -> Result<()> {
    use crate::session::Storage;

    let current_tmux = crate::tmux::get_current_session_name().unwrap_or_default();
    let peers = get_peer_sessions();
    let idx = args.index.saturating_sub(1);

    if let Some(peer) = peers.get(idx) {
        // Update last_user_activity for the target session (marks it as most recent)
        if let Ok(storage) = Storage::new(&peer.profile) {
            if let Ok((mut instances, groups)) = storage.load_with_groups() {
                if let Some(inst) = instances.iter_mut().find(|i| i.id == peer.session_id) {
                    inst.last_user_activity = Some(chrono::Utc::now());
                }
                let tree = crate::session::GroupTree::new_with_groups(&instances, &groups);
                let _ = storage.save_with_groups(&instances, &tree);
            }
        }

        // Apply tmux status bar to the target session
        crate::tmux::status_bar::apply_all_tmux_options(&peer.tmux_name, &peer.title, None, None);

        // Build peers for the TARGET session: exclude target, include source
        let target_peers = format_peers_for_session(&peer.tmux_name);
        set_peers_on_session(&peer.tmux_name, &target_peers);

        // Also refresh peers on the SOURCE session for when user returns
        if !current_tmux.is_empty() {
            let source_peers = format_peers_for_session(&current_tmux);
            set_peers_on_session(&current_tmux, &source_peers);
        }

        // Switch tmux client to the target session
        let _ = std::process::Command::new("tmux")
            .args(["switch-client", "-t", &peer.tmux_name])
            .status();

        // Force immediate status bar redraw on the now-active client
        let _ = std::process::Command::new("tmux")
            .args(["refresh-client", "-S"])
            .output();
    }

    Ok(())
}

/// Compute formatted peers string from a given session's perspective.
fn format_peers_for_session(exclude_tmux_name: &str) -> String {
    use crate::hooks::read_hook_status;
    use crate::session::{list_profiles, GroupTree, Status, Storage};
    use crate::tmux;

    let profiles = list_profiles().unwrap_or_else(|_| vec!["default".to_string()]);
    let mut peers: Vec<(PeerSession, Option<chrono::DateTime<chrono::Utc>>)> = Vec::new();

    for profile in &profiles {
        let storage = match Storage::new(profile) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (instances, groups) = match storage.load_with_groups() {
            Ok(r) => r,
            Err(_) => continue,
        };
        let tree = GroupTree::new_with_groups(&instances, &groups);

        for inst in &instances {
            let tmux_name = tmux::Session::generate_name(&inst.id, &inst.title);
            if tmux_name == exclude_tmux_name {
                continue;
            }
            let session = tmux::Session::new(&inst.id, &inst.title);
            if !session.as_ref().map(|s| s.exists()).unwrap_or(false) {
                continue;
            }
            let status = read_hook_status(&inst.id).unwrap_or(Status::Idle);
            let icon = match status {
                Status::Running => "\u{25cf}",
                Status::Waiting => "\u{25d0}",
                _ => "\u{25cb}",
            };
            let group_prefix = if inst.group_path.is_empty() {
                None
            } else {
                Some(
                    tree.get_group(&inst.group_path)
                        .map(|g| g.display_prefix())
                        .unwrap_or_else(|| {
                            inst.group_path
                                .rsplit('/')
                                .next()
                                .unwrap_or(&inst.group_path)
                                .chars()
                                .take(3)
                                .collect::<String>()
                                .to_uppercase()
                        }),
                )
            };
            peers.push((
                PeerSession {
                    title: inst.title.clone(),
                    icon,
                    tmux_name,
                    session_id: inst.id.clone(),
                    profile: profile.clone(),
                    group_prefix,
                },
                inst.last_user_activity,
            ));
        }
    }

    peers.sort_by(|a, b| {
        let a_time = a.1.unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC);
        let b_time = b.1.unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC);
        b_time.cmp(&a_time)
    });

    let peer_list: Vec<PeerSession> = peers.into_iter().take(5).map(|(p, _)| p).collect();
    format_peers_string(&peer_list)
}
