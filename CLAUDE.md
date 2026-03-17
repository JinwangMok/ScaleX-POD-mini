# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Overview

**Unified multi-cluster Kubernetes provisioning** repo using a 5-layer SDI architecture:
Physical (4 bare-metal) → SDI (OpenTofu virtualization) → Node Pools → Cluster (Kubespray) → GitOps (ArgoCD)

**Primary CLI**: `scalex` (Rust, in `scalex-cli/`) — handles facts gathering, SDI provisioning, multi-cluster Kubespray, and resource queries.

**Automated Install**: `bash install.sh --auto` — fully unattended E2E from clean state (~45 min). Handles sudo keepalive, SSH API tunnels, namespace auto-creation, and secrets for both management and workload roles. Resume-safe (skips completed phases).

## Architecture

- **Tower cluster**: Management cluster (ArgoCD, Keycloak, Cloudflare Tunnel). Provisioned via Kubespray on SDI VMs.
- **Sandbox cluster**: Workload cluster. Provisioned via Kubespray on SDI VMs or bare-metal nodes.
- **All clusters use Kubespray** (production-grade). No k3s.
- **External access**: Cloudflare Tunnel + Tailscale. LAN access via switch.
- **External DNS pattern**: API endpoints follow `{cluster}-api.jinwang.dev` (e.g., `tower-api.jinwang.dev`). Single-level subdomain required — CF free tier wildcard certs only cover `*.jinwang.dev`, not `*.*.jinwang.dev`.

## CLI (`scalex`)

```bash
# Build
cd scalex-cli && cargo build --release

# Hardware facts gathering
scalex facts --all                       # Gather all node hardware info
scalex facts --host playbox-0            # Single node

# SDI (Software-Defined Infrastructure)
scalex sdi init                          # Virtualize all bare-metal → resource pool
scalex sdi init <sdi-specs.yaml>         # Create VM pools from spec
scalex sdi clean --hard --yes-i-really-want-to  # Full reset
scalex sdi sync                          # Reconcile bare-metal changes

# Cluster provisioning
scalex cluster init <k8s-clusters.yaml>  # Kubespray → multi-cluster

# Resource queries
scalex get baremetals                    # Hardware facts table
scalex get sdi-pools                     # VM pool status
scalex get clusters                      # Cluster inventory
scalex get config-files                  # Config file validation
```

### Config Files

| File | Purpose |
|------|---------|
| `credentials/.baremetal-init.yaml` | SSH access to bare-metal nodes (user-provided) |
| `credentials/.env` | SSH passwords/key paths (user-provided) |
| `credentials/secrets.yaml` | Keycloak, ArgoCD, Cloudflare secrets |
| `config/sdi-specs.yaml` | VM pool definitions (CPU, RAM, disk, GPU) |
| `config/k8s-clusters.yaml` | Cluster definitions (mode, role, addons) |

## Testing

```bash
# Rust CLI tests (782 tests)
cd scalex-cli && cargo test
cargo clippy                             # Lint
cargo fmt --check                        # Format check

# All tests + YAML lint
./tests/run-tests.sh
```

## Dashboard (`scalex dash`)

```bash
scalex dash                              # Interactive TUI (ratatui) — multi-cluster overview
scalex dash --headless                   # JSON output for scripting/verification
scalex dash --headless --resource pods   # Filter by resource type (pods, nodes, configmaps)
```

- Displays clusters, node health, pod status, and resource capacity across all kubeconfigs in `_generated/`
- **4-strategy cluster discovery**: `discover_clusters_streaming()` in `kube_client.rs` tries per cluster in parallel via `tokio::spawn`: (1) `api_endpoint` domain URL + SA bearer token (CF Tunnel path) → (2) kubeconfig original IP → (2b) `.original` kubeconfig VM IP fallback when primary kubeconfig has an unreachable domain URL → (3) SSH tunnel via bastion (500ms settle wait). Each probe has a 3s timeout. Each connected `ClusterClient` carries `server_version` (fetched with 500ms timeout via `client.apiserver_version()`, `None` on failure) and `endpoint` (the URL that succeeded).
- **`api_endpoint` field**: Optional field on `ClusterDef` (`models/cluster.rs`). Set in `config/k8s-clusters.yaml` to provide a stable external URL (e.g., Cloudflare Tunnel domain) for dash connectivity without relying on LAN IPs or SSH tunnels.
- After `install.sh`, `scalex dash` works without manual tunnel setup
- `metrics_server_enabled` is hardcoded `false` in `kubespray.rs` — metrics utilization bars show N/A until enabled
- **Skeleton startup**: TUI draws immediately on launch before first data fetch
- **Full prefetch for selected cluster**: Selected cluster always fetches ALL resource types (pods, deployments, services, configmaps, nodes, namespaces) in parallel on every fetch cycle. This makes view switching (p/d/s/c/n) instant — no network fetch needed since all data is cached. Non-selected clusters only fetch namespaces + nodes (for health dots + status bar). Each API call has a 2s `tokio::time::timeout` (`API_CALL_TIMEOUT` in `data.rs`). Headless mode (`--headless`) fetches all clusters in parallel with full resources.
- **Selected-cluster-only fetch on navigation**: View switch (p/d/s/c/n), cluster select, and namespace select set `refresh_selected_only=true`, which skips non-selected clusters entirely. Timer refresh and manual refresh (`r` key) fetch all clusters. View switch skips fetch entirely if the target resource is already in `fetched_resources`.
- **Incremental snapshot merge**: fetch results are merged per-cluster into existing `ClusterSnapshot` — selected cluster gets all resource fields updated; non-selected clusters only get namespaces + nodes updated (preserving cached pods/deployments/etc). Health/resource_usage are recomputed on every merge using the freshest available pods + nodes.
- **Event-driven input loop**: TUI uses `tokio::select!` with crossterm `EventStream` instead of 100ms polling. Keyboard input has near-zero latency. Tick interval (100ms) drives spinner animation and periodic refresh checks. Biased select prioritizes keyboard over ticks.
- **O(1) snapshot lookup**: `snapshot_index: HashMap<String, usize>` maps cluster name → position in `snapshots` vec. Updated inline during fetch merge. `current_snapshot()` uses index with name-guard + linear scan fallback.
- **Pre-lowercased search**: `search_query_lower: Option<String>` synced once per event cycle via `sync_search_lower()`. Eliminates per-item `to_lowercase()` on query string during search filtering.
- **Zero-allocation search matching**: `contains_ignore_ascii_case()` performs char-by-char ASCII comparison without allocating `to_lowercase()` strings per item. K8s resource names are always ASCII.
- **Viewport-only sidebar rendering**: `render_sidebar` only builds `Line` objects for visible viewport rows (`scroll_offset..scroll_offset+height`), not all tree nodes. Combined with single snapshot lookup per cluster node (health dot + namespace count reuse same lookup).
- **Cached visible tree data**: `cached_visible_len: Option<usize>` and `cached_visible_indices: Option<Vec<usize>>` avoid redundant O(n) tree scans across multiple callers per frame. Invalidated at start of each `handle_event`, after tree mutations (drain, splice), and after expansion state changes.
- **Pre-computed display strings**: All integer-to-string conversions are done once during fetch, not per-frame in render. `NodeInfo` carries `roles_display`, `mem_capacity_display`, `mem_allocatable_display`, `cpu_display`, `mem_display`. `PodInfo` carries `restarts_display`. `DeploymentInfo` carries `up_to_date_display`, `available_display`. `ConfigMapInfo` carries `data_keys_display`.
- **Search cleared on context switch**: Selecting a cluster or namespace via `Enter` clears active search filter (`search_query` and `search_query_lower`) to prevent stale filters applying to new context.
- **Dirty-flag redraw**: `needs_redraw: bool` skips `terminal.draw()` when nothing changed. Set true by keyboard events, fetch results, discovery events, terminal resize. Tick sets it only when spinner is visible (`is_fetching || !discover_complete`). Eliminates ~90% of unnecessary terminal I/O when idle.
- **Pre-computed cluster labels**: `TreeNode.ns_count_label` caches "name (Nns)" format, computed during `sync_tree_from_snapshots`. Used by `render_sidebar` instead of per-frame `format!()` for expanded cluster nodes.
- **Top tab viewport-aware scroll**: `page_down`, `jump_end`, and `move_down` for Top tab (Paragraph scroll) clamp offset to `max(0, line_count - page_size)`, not `line_count - 1`. Prevents over-scrolling past content when viewport is larger than content.
- **Single-join parallel fetch**: `fetch_cluster_snapshot` runs all API calls (namespaces, nodes, pods, deployments, services, configmaps, events) in a single `tokio::join!` instead of two sequential groups. Eliminates sequential latency between namespace/node and resource fetches.
- **Zero-clone render visible indices**: `render_visible_indices: Vec<usize>` pre-computed once before `terminal.draw()` in `run_tui`. `render_sidebar` borrows directly (`&app.render_visible_indices`) instead of cloning `Vec<usize>` per frame.
- **Static sidebar indentation**: `render_sidebar` uses pre-computed static `&str` slices (`INDENTS` array) instead of per-row `"  ".repeat(depth)` allocation. Label references use `&str`/`Cow<str>` to avoid `String::clone()` in the common (non-truncated) case.
- **Index-based tree sync**: `sync_tree_from_snapshots` uses index-based iteration over `self.snapshots` to avoid cloning all snapshot names and namespace lists into a temporary `Vec`. Namespace change detection uses lockstep iterator comparison instead of collecting into `Vec<String>`.
- **Off-thread blocking I/O**: `read_self_rss_mb()` and `load_infra()` run on the tokio worker thread inside the fetch task, not on the main event loop. Results delivered via `FetchResult` fields (`self_rss_mb`, `infra`).
- **DeploymentInfo integer fields**: `ready_count` and `desired_count` stored as `i32` alongside the display string `ready`. Render path uses integers directly for READY column color coding — eliminates `Vec::new()` + 2 string parses per deployment row per frame.
- **Active selection marker on expanded clusters**: `is_active_selection` for `NodeType::Cluster` no longer requires `!node.expanded`. Marker `●` stays visible on the cluster node when selected with no namespace filter, even when the cluster tree is expanded.
- **Static usage bar strings**: `render_usage_bar` indexes into static `BAR_FILL`/`BAR_EMPTY` `&str` constants instead of `"=".repeat(filled)` / `"-".repeat(empty)`. Eliminates 2 heap allocations per cluster per frame in the status bar.
- **Cached context label**: `App.ctx_label` pre-computed on cluster/namespace change via `sync_ctx_label()`. `render_center` borrows `&app.ctx_label` instead of `format!()` per frame.
- **Node VERSION column**: `NodeInfo.kubelet_version` populated from `node.status.nodeInfo.kubeletVersion`. Shown in nodes table after ROLES column and in Top tab after node name. Useful for upgrade planning.
- **Service EXTERNAL-IP column**: `ServiceInfo.external_ip` populated from `status.loadBalancer.ingress[].ip/hostname`. Shows `<none>` for non-LB services. Column appears between CLUSTER-IP and PORTS.
- **Alphabetical resource sorting**: Deployments, services, configmaps, and nodes sorted by name after fetch. Pods retain severity-first sorting (CrashLoopBackOff first, then pending, running, completed). Events sorted by last_seen ascending (most recent first).
- **Reduced API timeouts**: `API_CALL_TIMEOUT` reduced from 2s to 500ms, `DISCOVER_TIMEOUT` from 3s to 2s. Healthy clusters respond in <200ms; tighter timeouts minimize worst-case fetch latency.
- **Zero-clone tree index lookups**: `tree_index_at_cursor()` reads from cached visible indices without cloning `Vec<usize>`. `ensure_visible_indices_cached()` populates cache; callers avoid `visible_tree_indices_cached()` clone where possible.
- **Static sidebar padding**: `render_sidebar` uses static `SPACES` buffer for row padding instead of per-row `" ".repeat(pad)` heap allocation.
- **Cached row count**: `cached_row_count: Option<usize>` avoids redundant O(n) filter iterations in `move_down`/`page_down`/`jump_end`/`render_center`. Invalidated per event cycle.
- **Pre-computed status bar strings**: `status_bar_health_strings` computed on fetch result arrival via `sync_status_bar_strings()`. `render_status_bar` reads pre-computed strings instead of `format!()` per snapshot per frame.
- **Viewport-only Row construction**: `render_resource_table` counts filtered items without collecting into `Vec<&T>`, then builds `Row` objects only for viewport-visible items via iterator `skip`/`take`.
- **Headless probe timeout**: Headless `discover_clusters` Strategy 2/2b uses `probe_client()` (with `DISCOVER_TIMEOUT`) instead of inline untimed probe.
- **Safe unwrap elimination**: `navigate_to_parent` and `visible_tree_indices_cached` use `if let`/`unwrap_or_default` instead of `unwrap()` on invariant-maintained `Option`.
- **Skip namespace fetch for non-selected clusters**: `ActiveResource::Nodes` arm fetches only nodes (no namespace API call). Merge logic preserves cached namespaces when fetch returns empty vec.
- **Parallel fetch result collection**: Per-cluster fetch handles collected via `futures::future::join_all` instead of sequential `for handle in handles { handle.await }`. Eliminates cancellation latency and serial blocking.
- **Zero-clone pre-draw visible indices**: `run_tui` pre-draw populates `render_visible_indices` via `extend_from_slice` from cache instead of `clone().unwrap()`. Reuses existing Vec capacity across frames.
- **Static tab shortcut labels**: `render_center` uses `&'static str` constants (`SHORTCUTS_ACTIVE`/`SHORTCUTS_INACTIVE`) instead of per-frame `format!("[{}]{} ", key, label)`. Eliminates 5 String allocations per frame.
- **Static health dot strings**: `render_status_bar` uses `"● "` / `"○ "` constants instead of per-cluster `format!("{} ", symbol)`.

### Header Layout

The TUI header is k9s-style and responsive:
- **Full mode** (terminal height ≥ 28): 8-line header with ASCII art `SCALEX` logo on the left and cluster info (Context, Cluster endpoint, K8s revision, ScaleX version + cluster count, config path) on the right. Logo hidden when terminal width < 82 columns.
- **Compact mode** (terminal height < 28): 4-line header with condensed info rows (no logo). No tab bar — active tab is inferred from center panel content.
- Header info sourced from active `ClusterClient`: `name` (Context), `endpoint` (Cluster), `server_version` (K8s Rev), `kubeconfig_path` (Config). Version from `env!("CARGO_PKG_VERSION")`.

### Keybindings

| Key | Action |
|-----|--------|
| `j`/`k` or arrows | Move cursor (does **not** change selected cluster/namespace) |
| `Enter` | Select cluster or namespace (sets active context for center panel) |
| `h`/`l` or arrows | Collapse / Expand node; Left on leaf/collapsed navigates to parent |
| `PgUp`/`PgDn` | Jump half viewport up/down |
| `Home`/`End` | Jump to first/last item |
| `Tab`/`Shift+Tab` | Cycle between Sidebar and Center panel |
| `1` `2` | Switch to tab (1=Resources, 2=Top) |
| `p` `d` `s` `c` `n` `e` | Switch resource view (works from both panels; from Sidebar also switches to Center) |
| `y` | Describe / YAML view for selected resource |
| `Shift+F` | Port forward selected pod/service |
| `Shift+L` | View pod logs (Pods view only) |
| `Shift+S` | Shell exec into pod (Pods view only) |
| `:` | Command mode (k9s-style resource navigation) |
| `/` | Enter search mode (filter by name and namespace) |
| `r` | Force data refresh + retry failed cluster connections |
| `?` | Toggle help overlay (context-sensitive: shows keys for current panel/view) |
| `ESC` | Close help overlay / Cancel search / Close modal |
| `q`/`Ctrl+C` | Quit |

### UX Design Invariants

- **Cursor ≠ selection**: `j`/`k` never changes `selected_cluster`/`selected_namespace`. Only `Enter` commits selection. `l`/Right expands without selecting.
- **Active selection indicator**: selected node shown with `●` marker + bold aqua, distinct from yellow cursor highlight.
- **Sidebar health dots**: connected clusters show colored health dot (● green/yellow/red, ○ unknown) as suffix. Discovering shows `[..]`, failed shows `[!!]`.
- **Sidebar namespace count**: expanded cluster labels show namespace count suffix like `tower (12ns)`.
- **Retry failed connections**: `r` key (and only `r`) re-spawns cluster discovery for failed connections via `discover_clusters_streaming_filtered`. View switches (`p`/`d`/`s`/`c`/`n`) do NOT trigger retry — only `retry_failed_clusters` flag controls this.
- **View switch triggers refresh**: `p`/`d`/`s`/`c`/`n` sets `needs_refresh=true` for immediate re-fetch. Works from both Sidebar and Center panel; from Sidebar also switches `active_panel` to Center.
- **No stale data**: full prefetch for selected cluster means all resource types are always up-to-date. `App::is_view_stale()` always returns `false`. The `[cached]` indicator is never shown.
- **Connection failure display**: if `cluster_connection_status` maps a cluster to `ConnectionStatus::Failed`, the center panel (both Resources and Top tabs) renders an error message with retry hint instead of the resource table. Sidebar shows `[!!]` suffix in red.
- **Stale fetch discard**: `App::fetch_generation` (u64 counter) is incremented on every navigation/view change. Each spawned fetch task captures the generation at launch; results are dropped if `result.generation != app.fetch_generation` on arrival, preventing stale overwrites.
- **Left navigates to parent**: `h`/Left on a leaf node (namespace, infra item) or already-collapsed node navigates cursor to its parent. Leaf nodes cannot expand/collapse.
- **Search matches name + namespace**: `/` search filters center table rows by both resource name and namespace (case-insensitive). Nodes view and Top tab filter by node name only.
- **k9s-style pod sorting**: Pods table sorted by status severity — errors/crashes first (CrashLoopBackOff, OOMKilled, Failed, Evicted), then pending/init, then running, then completed. Stable sort preserves order within each group. Sort applied during `fetch_cluster_snapshot` via `sort_pods_by_severity()`.
- **Status color coding**: Pod RESTARTS column: yellow (1-10), red (>10). Deployment READY column: green (ready≥desired), yellow (0<ready<desired), red (ready=0). Node roles show `<none>` when empty.
- **Full-width cursor highlight**: sidebar cursor highlight fills the entire row width, not just text length. Padding is computed using display-column widths (not byte lengths) to correctly handle Unicode markers (●, ▼, ▶, …).
- **Responsive sidebar width**: sidebar width adapts to terminal: 20 cols (<60), 24 cols (<80), 28 cols (≥80). Labels truncated with `…` when overflowing.
- **Sidebar scroll indicator**: `N/M` position indicator shown at bottom-left when sidebar content overflows viewport.
- **Table column constraints**: resource tables use `Min`/`Length` constraints instead of percentages for better narrow terminal support — fixed-width columns (READY, AGE, RESTARTS) don't shrink, flexible columns (NAME, NAMESPACE) absorb remaining space.
- **Shared tab preamble**: `render_tab_preamble()` deduplicates connection error + loading state rendering between Resources and Top tabs.
- **Node AGE column**: Nodes table shows AGE computed from node `creation_timestamp`, consistent with all other resource views.
- **Service nodePort display**: Service PORTS column shows `port:nodePort/proto` for NodePort/LoadBalancer services, `port/proto` for ClusterIP.
- **Status bar narrow terminal**: per-cluster CPU/MEM usage bars hidden when terminal width < 60 cols; self/latency info always shown.
- **Health computation**: `compute_health` counts `OOMKilled`, `ImagePullBackOff`, `ErrImagePull`, `Evicted` as failed pods. Uses `starts_with("Ready")` for nodes so `Ready,SchedulingDisabled` (cordoned) still counts as ready.
- **expand_node dispatch**: `l`/Right on Cluster calls only `sync_tree_from_snapshots`; on InfraHeader calls only `sync_infra_tree`; Root calls neither.
- **Stale fetch race safety**: stale fetch results (wrong generation) do NOT clear `is_fetching`/`fetch_started_at` — only the matching generation's result resets fetch state. Prevents duplicate API calls.
- **Init container status**: `derive_effective_status` checks init containers first — shows `Init:N/M` when init containers are not yet complete, or `Init:CrashLoopBackOff (name)` on init error.
- **Pod-level reason override**: `derive_effective_status` checks `status.reason` for `Evicted`, `NodeLost`, `Shutdown` before container-level checks.
- **Cordoned node display**: nodes with `spec.unschedulable=true` show `Ready,SchedulingDisabled` status, colored yellow (not green). Top tab shows filled dot (●) for cordoned-but-ready nodes.
- **Auto-select cursor alignment**: when first cluster auto-connects, `tree_cursor` moves to that cluster's tree index so visual cursor matches the selected context.
- **Human-readable memory**: `format_k8s_memory()` converts raw K8s quantities (e.g., `7816040Ki` → `7.5Gi`) in nodes table and Top tab. CPU values pass through unchanged.
- **No-metrics sentinel**: `compute_resource_usage` returns `-1.0` for CPU/MEM when no metrics-server data. `render_usage_bar` shows `N/A` for negative values. Top tab title shows `(no metrics)` suffix.
- **Discovery log channel**: `DiscoverEvent::Log { message }` replaces all `eprintln!` in streaming discovery paths to avoid TUI corruption. Messages stored in `app.discovery_logs` (capped at 10) and displayed in status bar with ~10s auto-fade. Headless mode (`discover_clusters()`) retains `eprintln!` since no TUI is active.
- **Domain-first kubeconfig**: `install.sh` `cleanup_api_tunnels()` rewrites kubeconfigs with `api_endpoint` domain URLs after CF Tunnel is healthy. Original VM IP kubeconfigs saved as `kubeconfig.yaml.original` for fallback. `scalex dash` Strategy 2b tries `.original` file when primary kubeconfig has a domain URL that is unreachable.
- **CF Tunnel SA token auth**: CF Tunnel cannot proxy mTLS client certs, so `build_client_with_endpoint()` strips kubeconfig CA + client cert and injects a ServiceAccount bearer token. SA `scalex-dash` in `scalex-system` namespace, bound to `view` ClusterRole. Token cached at `_generated/clusters/{name}/dash-token`. Auto-provisioned on first run via SSH through bastion if cached token absent. Module: `sa_provisioner.rs`. To re-provision: delete `dash-token` and relaunch.
- **k9s attribution**: help overlay (`?` key) footer shows "Inspired by k9s (github.com/derailed/k9s)" in DarkGray.
- **Cached data persistence**: `render_tab_preamble` returns cached snapshot even when `ConnectionStatus::Failed` — error shown as 1-line red banner via `render_connection_error_banner()`, not full-area replacement. Data stability: once displayed, data stays visible until next fetch result arrives.
- **Cached header info**: `HeaderInfo` struct (cluster_name, endpoint, k8s_version, config_path) pre-computed via `sync_header_info()` on cluster selection change and discovery events. `render_header` reads cached struct instead of O(n) `clusters.iter().find()` + `display().to_string()` per frame.
- **Pre-computed status bar self/latency**: `status_bar_self_line` caches `"| self: 42MB | latency: 150ms"` on fetch result arrival. Spinner appended dynamically only when fetching. Eliminates per-frame `format!()` for rss + latency.
- **Pre-computed top tab node display**: `NodeInfo.top_display` pre-computed during fetch as `"  v1.33.1  CPU: 8/8  MEM: 7.5Gi/7.8Gi"`. `render_top_tab` borrows pre-computed string instead of per-node `format!()` per frame.
- **Cached center panel title**: `ctx_title_span` pre-computed on cluster/namespace change via `sync_ctx_label()`. `render_center` borrows cached string instead of per-frame `format!("| {} ", ctx_label)`.
- **Static usage bar labels**: `render_usage_bar` uses static `label` + `" ["` spans instead of `format!("{} [", label)`. Eliminates 2 format allocations per bar per frame.
- **Headless parallel fetch**: `run_headless` uses `futures::future::join_all` instead of sequential `for handle in handles { handle.await }` for cluster data fetching.
- **Safe PID cast**: All `pid as i32` casts for `libc::kill` are guarded with `pid <= i32::MAX as u32` bounds check. Prevents negative PID values that could send signals to process groups.
- **Snapshot index rebuild**: `rebuild_snapshot_index()` called after fetch result merge loop to prevent stale index mappings when new clusters are added.
- **Zero-clone visible indices in run_tui**: Auto-select path uses `ensure_visible_indices_cached()` + direct cache reference instead of `visible_tree_indices_cached()` clone.
- **Iterator-based Top tab render**: `render_top_tab` iterates `snapshot.nodes.iter().filter()` directly instead of collecting into `Vec<&NodeInfo>` per frame.
- **Static spinner strings**: Status bar uses pre-computed `SPINNERS`, `DISCOVER_SPINNERS`, `LOADING_SPINNERS` static arrays instead of per-frame `format!()` for spinner animation.
- **Zero-allocation header strings**: `render_header_full` and `render_header_compact` pass `&str` references directly to `Span::styled` instead of `.to_string()` conversion per frame.
- **Safe tunnel port counter**: `NEXT_TUNNEL_PORT` uses `AtomicU32` instead of `AtomicU16` to prevent silent wrap-around at 65535 after many retry cycles.
- **Pre-computed cluster name labels**: `status_bar_health_strings` tuple includes pre-computed `"name: "` label. `render_status_bar` borrows cached string instead of `format!("{}: ", snapshot.name)` per cluster per frame.
- **Per-resource fetch tracking**: `fetched_resources: HashSet<ActiveResource>` distinguishes "not yet fetched" (empty vec, not in set) from "fetched but truly empty" (empty vec, in set). Cleared on cluster/namespace change. View switch to unfetched resource shows "Loading {type}..." spinner instead of empty table.
- **Events resource view**: `EventInfo` struct with namespace, event_type, reason, object (Kind/name), message, count, last_seen, age. Fetched via core/v1 Events API with `API_CALL_TIMEOUT`. `e` key switches to Events view. Warning events shown in yellow. Table columns: NAMESPACE, LAST SEEN, TYPE, REASON, OBJECT, MESSAGE. Events sorted by last_seen ascending. Search matches reason, namespace, object, and message.
- **Static preamble spinner strings**: `render_tab_preamble` and `render_resources_tab` loading indicators use static `[&str; 4]` arrays instead of per-frame `format!()` with spinner character. Static `&str` messages replace `.to_string()` on literals.
- **Static usage bar percent suffix**: `PERCENT_SUFFIXES` static `[&str; 101]` lookup table replaces per-frame `format!("] {:>3.0}% ", percent)` in `render_usage_bar`. Indexed by `percent.round() as usize`.
- **Dead code removed**: `is_view_stale()` method removed (always returned false with full prefetch). `[cached]` indicator rendering removed from `render_center`.
- **Events table uses generic renderer**: `render_events_table` uses `render_resource_table` generic with `resource_header()` for consistent header styling (BRIGHT_YELLOW+BG1), correct cursor highlight (panel-guarded via `row_base_style`), and single-pass filter iteration.
- **Cached row count in render**: `render_resource_table` reads `current_row_count_readonly()` (cached from event cycle) instead of re-counting filtered items per render call. Eliminates double iteration (count + render) in the table render hot path.
- **Pre-computed sidebar indicator**: `App.sidebar_indicator` caches `" pos/total "` string, synced before each draw via `sync_sidebar_indicator()`. `render_sidebar` borrows cached string instead of per-frame `format!()`.
- **Pre-computed row count indicator**: `App.row_count_indicator` caches `"pos/total "` string, synced before each draw via `sync_row_count_indicator()`. `render_center` borrows cached string instead of per-frame `format!()`.
- **Pre-computed header display strings**: `HeaderInfo.version_display`, `cluster_count_full`, `cluster_count_compact`, `version_compact` computed in `sync_header_info()`. `render_header_full` and `render_header_compact` borrow pre-computed strings instead of per-frame `format!()` for version and cluster count.
- **YAML/describe modal**: `y` key opens describe overlay for any resource. Shows cached summary instantly, then async-fetches full API describe via `describe_resource_yaml()` (3s timeout). Modal intercepts all navigation keys (j/k, PgUp/PgDn, Home/End). ESC closes and cancels in-flight fetch via generation counter.
- **Log viewer modal**: `Shift+L` opens pod log streaming overlay. Uses `kube::Api<Pod>::log_stream()` with `follow=true, tail_lines=100` via `futures::AsyncBufReadExt::lines()`. Lines delivered via `mpsc` channel with generation-based stale filtering. `f` toggles auto-follow. ESC closes and increments generation to cancel stream.
- **Command mode**: `:` activates k9s-style command bar with tab-autocomplete, history, fuzzy match. `ResourceRegistry` resolves aliases to API resources. Submission triggers dynamic resource fetch via `pending_dynamic_fetch` channel.
- **Port forward**: `Shift+F` opens port-picker modal for pods/services. `PortForwardManager` tracks active kubectl subprocesses with monitored status (Starting→Active→Stopped/Failed).
- **Modal overlay stacking**: Render order: help → port-picker → port-forward → yaml-modal → log-viewer → toasts. Event interception order matches render order (innermost modal gets priority). All modals close on ESC/q.
- **Async request pattern**: `pending_*` fields on App queue one-shot requests consumed by `run_tui` event loop. Generation counters discard stale results. Channels: `log_line_tx/rx` (256 cap), `describe_tx/rx` (4 cap), `dyn_fetch_tx/rx` (8 cap).
- **Shell exec**: `Shift+S` on pod row suspends TUI (`tui_suspend::suspend_tui`), spawns `kubectl exec -it pod -n ns -- /bin/sh` as child process, resumes TUI on exit (`tui_suspend::restore_tui`). `ShellExecRequest` queued via `pending_shell_exec`, consumed by `run_tui` synchronously. Non-pod views show toast.
- **Cross-cluster mode**: `:resource --all` fetches resource from ALL connected clusters in parallel. `CommandMode` parses `--all` flag. CLUSTER column prepended to table. Objects tagged with `scalex.io/cluster` label during fetch. `cross_cluster_mode: bool` on App controls accumulation vs replacement of fetch results. `pending_cross_cluster_fetches: Vec<DynamicFetchRequest>` queues one request per cluster.

## Key Patterns

- **GitOps-First**: Post-bootstrap, ArgoCD manages all cluster state via ApplicationSets.
- **Sync waves**: 0=ArgoCD/cluster-config, 1=Cilium/cert-manager/Kyverno/local-path-provisioner, 2=cilium-resources/cert-issuers/kyverno-policies, 3=tunnel/keycloak, 4=RBAC.
- **Idempotent**: Every CLI operation safe to re-run.
- **Pure Functions**: Rust CLI uses pure functions for HCL/inventory/vars generation. No side effects in generators.
- **Secrets**: Created by CLI, stored in `credentials/` (gitignored). Templates in `credentials/*.example`.
- **Auto-SAN from `api_endpoint`**: `generate_cluster_vars()` auto-appends `supplementary_addresses_in_ssl_keys` when `api_endpoint` contains a DNS hostname (not an IP). Ensures K8s API server cert includes external domain for CF Tunnel TLS verification.
- **Generated output**: `_generated/` (gitignored) holds SDI HCL, kubespray inventory, kubeconfigs.

## GitOps Pattern

**Bootstrap**: `scalex bootstrap` (internally: Helm Cilium install on all clusters → Helm ArgoCD install → cluster register → `kubectl apply -f gitops/bootstrap/spread.yaml`)

**Multi-cluster structure**:
- `spread.yaml` → creates `tower-root` + `sandbox-root` Applications
- Each root points to `gitops/generators/{tower,sandbox}/`
- Generators deploy apps from `gitops/{common,tower,sandbox}/`

| Concept | ArgoCD Resource | Path |
|---------|----------------|------|
| **Projects** | AppProject | `gitops/projects/{tower,sandbox}-project.yaml` |
| **Generators** | ApplicationSet | `gitops/generators/{tower,sandbox}/` |
| **Common Apps** | Kustomization | `gitops/common/{cilium-resources,cert-manager,kyverno,kyverno-policies}/` |
| **Tower Apps** | Kustomization | `gitops/tower/{argocd,cilium,cert-issuers,cloudflared-tunnel,cluster-config,keycloak}/` |
| **Sandbox Apps** | Kustomization | `gitops/sandbox/{cilium,cluster-config,local-path-provisioner,rbac,test-resources}/` |

**Adding a new common app**: (1) Create `gitops/common/{app}/kustomization.yaml`, (2) Add element to both `gitops/generators/tower/common-generator.yaml` and `gitops/generators/sandbox/common-generator.yaml`.

**Adding a cluster-specific app**: (1) Create `gitops/{tower|sandbox}/{app}/kustomization.yaml`, (2) Add element to `gitops/generators/{tower|sandbox}/{tower|sandbox}-generator.yaml`.

## Coding Style

- **Rust**: Pure functions, no side effects in generators. `thiserror` for errors, `clap` derive for CLI.
- **YAML**: 2-space indent, double quotes for variables/IPs, kebab-case resource names.

## Project Structure

```
├── scalex-cli/                # Rust CLI (primary) — facts, SDI, cluster, get, status, kernel-tune, secrets
├── gitops/                    # ArgoCD-managed GitOps (multi-cluster)
│   ├── bootstrap/spread.yaml  # Root bootstrap (tower-root + sandbox-root)
│   ├── generators/            # ApplicationSets per cluster
│   │   ├── tower/             # common-generator + tower-generator
│   │   └── sandbox/           # common-generator + sandbox-generator
│   ├── projects/              # AppProjects (tower-project, sandbox-project)
│   ├── common/                # Apps for ALL clusters (cilium-resources, cert-manager, kyverno, kyverno-policies)
│   ├── tower/                 # Tower-only apps (argocd, keycloak, cloudflared-tunnel, ...)
│   └── sandbox/               # Sandbox-only apps (local-path-provisioner, rbac, ...)
├── credentials/               # Secrets + init config (gitignored, .example templates)
├── config/                    # User config templates (sdi-specs, k8s-clusters, baremetal)
├── ansible/                   # Node preparation playbooks
├── kubespray/                 # Kubespray submodule (v2.30.0) + templates
├── client/                    # OIDC kubeconfig generation
├── tests/                     # Test runner + YAML lint
├── docs/                      # Operations guide (Cloudflare, Keycloak, kernel, access)
└── _generated/                # Gitignored output (SDI HCL, inventories, kubeconfigs)
```
