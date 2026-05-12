use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use zellij_tile::prelude::*;

const DEFAULT_STORE_URL: &str = "zellij-agent-session-manager-store";
const TOP_PADDING: usize = 1;

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum Role {
    Store,
    #[default]
    Sidebar,
}

#[derive(Default)]
struct State {
    role: Role,
    tabs: Vec<TabInfo>,
    panes: HashMap<usize, Vec<PaneInfo>>,
    running_commands: BTreeMap<String, RunningCommand>,
    alerts: BTreeMap<usize, AlertCounts>,
    known_sidebars: BTreeSet<u32>,
    selected: usize,
    selected_tab_position: Option<usize>,
    own_pane_id: Option<PaneId>,
    last_focused_pane: Option<PaneId>,
    last_active_tab_id: Option<usize>,
    initialized_selection: bool,
    requested_initial_state: bool,
    was_sidebar_focused: bool,
    last_title: Option<String>,
    show_help: bool,
}

#[derive(Clone, Debug, Default)]
struct RunningCommand {
    command: String,
    tab_id: usize,
}

#[derive(Clone, Copy, Debug)]
struct TabContext {
    tab_id: usize,
    focused: bool,
}

#[derive(Clone)]
enum Row {
    Tab {
        position: usize,
        tab_id: usize,
        name: String,
        active: bool,
        alert: AlertCounts,
        alert_section: bool,
    },
}

#[derive(Deserialize)]
struct AgentPayload {
    pane_id: Option<String>,
    kind: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Snapshot {
    alerts: BTreeMap<usize, AlertCounts>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
struct AlertCounts {
    generic: usize,
    opencode_done: usize,
    opencode_waiting: usize,
}

#[derive(Clone, Copy, Debug)]
enum AlertKind {
    Generic,
    OpencodeDone,
    OpencodeWaiting,
}

impl AlertCounts {
    fn total(self) -> usize {
        self.generic + self.opencode_done + self.opencode_waiting
    }

    fn is_empty(self) -> bool {
        self.total() == 0
    }

    fn bump(&mut self, kind: AlertKind) {
        match kind {
            AlertKind::Generic => self.generic += 1,
            AlertKind::OpencodeDone => self.opencode_done += 1,
            AlertKind::OpencodeWaiting => self.opencode_waiting += 1,
        }
    }

    fn icon(self) -> &'static str {
        if self.opencode_waiting > 0 {
            "⚑"
        } else if self.opencode_done > 0 {
            "✦"
        } else {
            "●"
        }
    }

    fn marker(self) -> String {
        let icon = self.icon();
        let total = self.total();
        if total > 1 {
            format!("{} ({})", icon, total)
        } else {
            icon.to_string()
        }
    }
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        let default_role = if cfg!(feature = "store-default") {
            Role::Store
        } else {
            Role::Sidebar
        };
        self.role = match configuration.get("role").map(String::as_str) {
            Some("store") => Role::Store,
            Some("sidebar") => Role::Sidebar,
            _ => default_role,
        };

        let ids = get_plugin_ids();
        self.own_pane_id = Some(PaneId::Plugin(ids.plugin_id));

        match self.role {
            Role::Store => {
                set_selectable(false);
                request_permission(&[
                    PermissionType::ReadApplicationState,
                    PermissionType::ChangeApplicationState,
                    PermissionType::ReadCliPipes,
                    PermissionType::MessageAndLaunchOtherPlugins,
                ]);
                subscribe(&[
                    EventType::PermissionRequestResult,
                    EventType::TabUpdate,
                    EventType::PaneUpdate,
                    EventType::CommandChanged,
                ]);
            }
            Role::Sidebar => {
                set_selectable(true);
                show_cursor(None);
                request_permission(&[
                    PermissionType::ReadApplicationState,
                    PermissionType::ChangeApplicationState,
                    PermissionType::MessageAndLaunchOtherPlugins,
                ]);
                subscribe(&[
                    EventType::PermissionRequestResult,
                    EventType::TabUpdate,
                    EventType::PaneUpdate,
                    EventType::Key,
                    EventType::Mouse,
                ]);
            }
        }
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::TabUpdate(tabs) => {
                self.tabs = tabs;
                match self.role {
                    Role::Store => self.store_after_tabs_changed(),
                    Role::Sidebar => self.sidebar_after_state_update(),
                }
                true
            }
            Event::PaneUpdate(panes) => {
                self.panes = panes.panes;
                if self.role == Role::Sidebar {
                    self.sidebar_after_state_update();
                }
                true
            }
            Event::CommandChanged(pane_id, command, _, _) => {
                if self.role == Role::Store {
                    self.handle_command_changed(pane_id, command);
                    true
                } else {
                    false
                }
            }
            Event::Key(key) => {
                if self.role == Role::Sidebar {
                    self.handle_key(key);
                    true
                } else {
                    false
                }
            }
            Event::Mouse(mouse) => {
                if self.role == Role::Sidebar {
                    self.handle_mouse(mouse);
                    true
                } else {
                    false
                }
            }
            Event::PermissionRequestResult(_) => {
                if self.role == Role::Sidebar {
                    self.request_state_once();
                }
                true
            }
            _ => false,
        }
    }

    fn pipe(&mut self, message: PipeMessage) -> bool {
        match self.role {
            Role::Store => self.store_pipe(message),
            Role::Sidebar => self.sidebar_pipe(message),
        }
    }

    fn render(&mut self, _rows: usize, cols: usize) {
        if self.role == Role::Store {
            return;
        }
        self.render_expanded(cols);
    }
}

impl State {
    fn store_pipe(&mut self, message: PipeMessage) -> bool {
        match message.name.as_str() {
            "opencode.waiting" => {
                if let Some(payload) = message.payload {
                    self.apply_agent_payload(&payload, AlertKind::OpencodeWaiting);
                }
                false
            }
            "opencode.done" | "opencode.idle" | "opencode.status" => {
                if let Some(payload) = message.payload {
                    self.apply_agent_payload(&payload, AlertKind::OpencodeDone);
                }
                false
            }
            "opencode-sidebar.state.request" => {
                if let PipeSource::Plugin(plugin_id) = message.source {
                    self.known_sidebars.insert(plugin_id);
                    self.send_snapshot_to(plugin_id);
                }
                false
            }
            "opencode-sidebar.clear-tab" => {
                if let Some(payload) = message.payload {
                    if let Ok(tab_id) = payload.parse::<usize>() {
                        self.clear_tab_alert(tab_id);
                    }
                }
                false
            }
            "opencode-sidebar.goto-index" => {
                if let Some(payload) = message.payload {
                    if let Ok(index) = payload.parse::<usize>() {
                        self.goto_sidebar_index(index);
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn sidebar_pipe(&mut self, message: PipeMessage) -> bool {
        match message.name.as_str() {
            "opencode-sidebar.state.sync" => {
                if let Some(payload) = message.payload {
                    if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&payload) {
                        self.alerts = snapshot
                            .alerts
                            .into_iter()
                            .filter(|(_, alert)| !alert.is_empty())
                            .collect();
                        self.restore_selected_row();
                        self.update_own_title_if_needed();
                    }
                }
                true
            }
            "opencode-sidebar.focus" => {
                self.focus_self_if_active_tab();
                true
            }
            "opencode-sidebar.activate" => {
                self.activate_selected();
                true
            }
            "opencode-sidebar.local-clear-tab" => {
                if let Some(payload) = message.payload {
                    if let Ok(tab_id) = payload.parse::<usize>() {
                        self.alerts.remove(&tab_id);
                        self.restore_selected_row();
                        self.update_own_title_if_needed();
                    }
                }
                true
            }
            "opencode-sidebar.goto-index" => {
                if self.is_active_tab_sidebar() {
                    if let Some(payload) = message.payload {
                        if let Ok(index) = payload.parse::<usize>() {
                            self.goto_sidebar_index(index);
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn store_after_tabs_changed(&mut self) {
        let live_tab_ids: BTreeSet<usize> = self.tabs.iter().map(|tab| tab.tab_id).collect();
        let alert_count = self.alerts.len();
        self.alerts
            .retain(|tab_id, _| live_tab_ids.contains(tab_id));

        let mut changed = self.alerts.len() != alert_count;
        let active_tab_id = self.active_tab_id();
        if active_tab_id != self.last_active_tab_id {
            if let Some(active_tab_id) = active_tab_id {
                changed |= self.alerts.remove(&active_tab_id).is_some();
            }
            self.last_active_tab_id = active_tab_id;
        }

        if changed {
            self.broadcast_snapshot();
            self.update_own_title_if_needed();
        }
    }

    fn sidebar_after_state_update(&mut self) {
        let active_position = self.active_tab_position();
        if active_position != self.last_active_tab_position() {
            let previous_active_tab_id = self.last_active_tab_id;
            let active_tab_id = self.active_tab_id();
            self.select_active_tab_row();
            self.last_active_tab_id = active_tab_id;
            if let Some(active_tab_id) = active_tab_id {
                if Some(active_tab_id) != previous_active_tab_id
                    || self.alerts.contains_key(&active_tab_id)
                {
                    self.clear_local_and_store_tab_alert(active_tab_id);
                }
            }
        } else if let Some(position) = self.selected_tab_position {
            self.select_tab_row(position);
        }

        if let (Some(active_position), Some(active_tab_id)) =
            (active_position, self.active_tab_id())
        {
            if self.own_pane_in_tab(active_position) && self.alerts.contains_key(&active_tab_id) {
                self.clear_local_and_store_tab_alert(active_tab_id);
            }
        }

        let own_focused = active_position
            .and_then(|position| self.panes.get(&position))
            .map(|panes| {
                panes
                    .iter()
                    .any(|pane| self.is_own_pane(pane) && pane.is_focused)
            })
            .unwrap_or(false);
        if own_focused && !self.was_sidebar_focused {
            self.select_active_tab_row();
        }
        self.was_sidebar_focused = own_focused;

        if let Some(position) = active_position {
            if let Some(pane) = self.panes.get(&position).and_then(|panes| {
                panes
                    .iter()
                    .find(|pane| pane.is_focused && self.is_work_pane(pane))
            }) {
                self.last_focused_pane = Some(pane_id(pane));
            }
        }

        if !self.initialized_selection {
            self.select_active_tab_row();
            self.initialized_selection = true;
        }

        let row_count = self.rows().len();
        if self.selected >= row_count {
            self.selected = row_count.saturating_sub(1);
        }
        self.update_own_title_if_needed();
        self.request_state_once();
    }

    fn apply_agent_payload(&mut self, payload: &str, fallback_kind: AlertKind) {
        let Ok(payload) = serde_json::from_str::<AgentPayload>(payload) else {
            return;
        };
        let Some(pane_id) = payload.pane_id else {
            return;
        };
        let Some(tab_context) = self.tab_context_for_pane_key(&normalize_pane_key(&pane_id)) else {
            return;
        };
        let kind = match payload.kind.as_deref() {
            Some("waiting") => AlertKind::OpencodeWaiting,
            Some("done") => AlertKind::OpencodeDone,
            _ => fallback_kind,
        };
        let should_alert = if matches!(kind, AlertKind::OpencodeDone | AlertKind::OpencodeWaiting) {
            !(tab_context.focused && self.active_tab_id() == Some(tab_context.tab_id))
        } else {
            self.active_tab_id()
                .map(|active_tab_id| tab_context.tab_id != active_tab_id)
                .unwrap_or(true)
        };
        if should_alert {
            self.bump_tab_alert(tab_context.tab_id, kind);
        }
    }

    fn handle_command_changed(&mut self, pane_id: PaneId, command: Vec<String>) {
        let command = command_display(&command);
        if command.is_empty() {
            return;
        }

        let key = pane_key(&pane_id);
        let tab_context = self.tab_context_for_pane_key(&key);
        let current_is_shell = is_shell_command(&command);

        if let Some(previous) = self.running_commands.get(&key).cloned() {
            if previous.command != command && !is_shell_command(&previous.command) {
                let should_alert = self
                    .active_tab_id()
                    .map(|active_tab_id| previous.tab_id != active_tab_id)
                    .unwrap_or(true);
                if should_alert {
                    self.bump_tab_alert(previous.tab_id, AlertKind::Generic);
                }
            }
        }

        if current_is_shell {
            self.running_commands.remove(&key);
            return;
        }

        if let Some(tab_context) = tab_context {
            self.running_commands.insert(
                key,
                RunningCommand {
                    command,
                    tab_id: tab_context.tab_id,
                },
            );
        }
    }

    fn bump_tab_alert(&mut self, tab_id: usize, kind: AlertKind) {
        self.alerts.entry(tab_id).or_default().bump(kind);
        self.restore_selected_row();
        self.broadcast_snapshot();
        self.update_own_title_if_needed();
    }

    fn clear_tab_alert(&mut self, tab_id: usize) {
        if self.alerts.remove(&tab_id).is_some() {
            self.broadcast_snapshot();
            self.update_own_title_if_needed();
        }
    }

    fn goto_sidebar_index(&mut self, index: usize) {
        let Some(row) = self.rows().get(index.saturating_sub(1)).cloned() else {
            return;
        };
        match row {
            Row::Tab {
                position, tab_id, ..
            } => {
                self.clear_local_and_store_tab_alert(tab_id);
                switch_tab_to((position + 1) as u32);
            }
        }
    }

    fn clear_local_and_store_tab_alert(&mut self, tab_id: usize) {
        self.alerts.remove(&tab_id);
        self.restore_selected_row();
        self.send_clear_tab(tab_id);
        self.broadcast_local_clear_tab(tab_id);
        self.update_own_title_if_needed();
    }

    fn restore_selected_row(&mut self) {
        if let Some(position) = self.selected_tab_position {
            self.select_tab_row(position);
            return;
        }
        let row_count = self.rows().len();
        if self.selected >= row_count {
            self.selected = row_count.saturating_sub(1);
        }
    }

    fn send_snapshot_to(&self, plugin_id: u32) {
        if let Ok(payload) = serde_json::to_string(&Snapshot {
            alerts: self.alerts.clone(),
        }) {
            pipe_message_to_plugin(
                MessageToPlugin::new("opencode-sidebar.state.sync")
                    .with_destination_plugin_id(plugin_id)
                    .with_payload(payload),
            );
        }
    }

    fn broadcast_snapshot(&self) {
        for plugin_id in &self.known_sidebars {
            self.send_snapshot_to(*plugin_id);
        }
    }

    fn request_state(&self) {
        pipe_message_to_plugin(
            MessageToPlugin::new("opencode-sidebar.state.request")
                .with_plugin_url(self.store_url()),
        );
    }

    fn request_state_once(&mut self) {
        if self.requested_initial_state {
            return;
        }
        if self.own_plugin_url().is_none() {
            return;
        }
        self.requested_initial_state = true;
        self.request_state();
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut tabs = self.tabs.clone();
        tabs.sort_by_key(|tab| tab.position);

        for tab in tabs.iter() {
            let alert = self.alerts.get(&tab.tab_id).copied().unwrap_or_default();
            if !alert.is_empty() {
                rows.push(Row::Tab {
                    position: tab.position,
                    tab_id: tab.tab_id,
                    name: tab.name.clone(),
                    active: tab.active,
                    alert,
                    alert_section: true,
                });
            }
        }

        for tab in tabs {
            let alert = self.alerts.get(&tab.tab_id).copied().unwrap_or_default();
            if !alert.is_empty() {
                continue;
            }
            rows.push(Row::Tab {
                position: tab.position,
                tab_id: tab.tab_id,
                name: tab.name.clone(),
                active: tab.active,
                alert,
                alert_section: false,
            });
        }
        rows
    }

    fn render_expanded(&self, cols: usize) {
        if self.show_help {
            self.render_help(cols);
            return;
        }
        for _ in 0..TOP_PADDING {
            println!("{}", fit("", cols));
        }

        for (index, row) in self.rows().iter().enumerate() {
            let display_index = index + 1;
            match row {
                Row::Tab {
                    name,
                    active,
                    alert,
                    alert_section,
                    ..
                } => {
                    let selected = index == self.selected;
                    let marker = if selected { "›" } else { " " };
                    if *alert_section {
                        let icon = alert.marker();
                        print_row(
                            vec![
                                (marker.to_string(), "\u{1b}[35;1m"),
                                (display_index.to_string(), "\u{1b}[36m"),
                                (format!(" {} ", icon), icon_style(*alert)),
                                (name.clone(), "\u{1b}[37m"),
                            ],
                            cols,
                            selected,
                        );
                    } else {
                        let active_marker = if *active { "▸" } else { " " };
                        let name_style = if *active {
                            "\u{1b}[34;1m"
                        } else {
                            "\u{1b}[37m"
                        };
                        print_row(
                            vec![
                                (marker.to_string(), "\u{1b}[35;1m"),
                                (display_index.to_string(), "\u{1b}[36m"),
                                (format!(" {} ", active_marker), "\u{1b}[32;1m"),
                                (name.clone(), name_style),
                            ],
                            cols,
                            selected,
                        );
                    }
                }
            }
        }
    }

    fn render_help(&self, cols: usize) {
        for _ in 0..TOP_PADDING {
            println!("{}", fit("", cols));
        }
        println!("{}", bold(&fit(" Keys", cols)));
        println!("{}", fit("", cols));
        println!("{}", fit(" j/k, arrows  move", cols));
        println!("{}", fit(" Enter       focus tab", cols));
        println!("{}", fit(" c           clear alert", cols));
        println!("{}", fit(" q/Esc       return", cols));
        println!("{}", fit(" ?           close help", cols));
        println!("{}", fit("", cols));
        println!("{}", dim(&fit(" Alerts", cols)));
        println!("{}", fit(" ⚑ input needed", cols));
        println!("{}", fit(" ✦ answer ready", cols));
        println!("{}", fit(" ● command done", cols));
    }

    fn handle_key(&mut self, key: KeyWithModifier) {
        if self.show_help {
            match key.bare_key {
                BareKey::Char('?') | BareKey::Esc | BareKey::Char('q') => self.show_help = false,
                _ => {}
            }
            return;
        }
        match key.bare_key {
            BareKey::Char('j') | BareKey::Down => self.move_selection(1),
            BareKey::Char('k') | BareKey::Up => self.move_selection(-1),
            BareKey::Enter => self.activate_selected(),
            BareKey::Char('c') => self.clear_selected_alert(),
            BareKey::Char('?') => self.show_help = true,
            BareKey::Esc | BareKey::Char('q') => self.focus_last_work_pane(),
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: Mouse) {
        match mouse {
            Mouse::LeftClick(line, _) => {
                if self.show_help {
                    self.show_help = false;
                    return;
                }
                let row_index = line - TOP_PADDING as isize;
                if row_index >= 0 {
                    self.selected = row_index as usize;
                    self.activate_selected();
                }
            }
            Mouse::ScrollDown(_) => self.move_selection(1),
            Mouse::ScrollUp(_) => self.move_selection(-1),
            _ => {}
        }
    }

    fn activate_selected(&mut self) {
        let rows = self.rows();
        let Some(row) = rows.get(self.selected).cloned() else {
            return;
        };
        match row {
            Row::Tab {
                position, tab_id, ..
            } => {
                self.clear_local_and_store_tab_alert(tab_id);
                self.select_tab_row(position);
                switch_tab_to((position + 1) as u32);
                let target = self
                    .last_focused_pane
                    .filter(|pane_id| self.pane_in_tab(position, pane_id))
                    .or_else(|| self.first_work_pane_in_tab(position));
                if let Some(pane_id) = target {
                    focus_pane_with_id(pane_id, false, false);
                }
            }
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let count = self.rows().len();
        if count == 0 {
            return;
        }
        if delta < 0 {
            self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
        } else {
            self.selected = (self.selected + 1) % count;
        }
        if let Some(Row::Tab { position, .. }) = self.rows().get(self.selected) {
            self.selected_tab_position = Some(*position);
        }
    }

    fn clear_selected_alert(&mut self) {
        let rows = self.rows();
        let Some(Row::Tab { tab_id, .. }) = rows.get(self.selected).cloned() else {
            return;
        };
        self.clear_local_and_store_tab_alert(tab_id);
    }

    fn send_clear_tab(&self, tab_id: usize) {
        pipe_message_to_plugin(
            MessageToPlugin::new("opencode-sidebar.clear-tab")
                .with_plugin_url(self.store_url())
                .with_payload(tab_id.to_string()),
        );
    }

    fn broadcast_local_clear_tab(&self, tab_id: usize) {
        pipe_message_to_plugin(
            MessageToPlugin::new("opencode-sidebar.local-clear-tab")
                .with_payload(tab_id.to_string()),
        );
    }

    fn focus_last_work_pane(&self) {
        if let Some(pane_id) = self.last_focused_pane {
            focus_pane_with_id(pane_id, false, false);
        } else if let Some(position) = self.active_tab_position() {
            if let Some(pane_id) = self.first_work_pane_in_tab(position) {
                focus_pane_with_id(pane_id, false, false);
            }
        }
    }

    fn focus_self_if_active_tab(&self) {
        let Some(own_pane_id) = self.own_pane_id else {
            return;
        };
        let Some(active_position) = self.active_tab_position() else {
            return;
        };
        if self.own_pane_in_tab(active_position) {
            focus_pane_with_id(own_pane_id, false, false);
        }
    }

    fn is_active_tab_sidebar(&self) -> bool {
        self.active_tab_position()
            .map(|position| self.own_pane_in_tab(position))
            .unwrap_or(false)
    }

    fn select_active_tab_row(&mut self) {
        if let Some(position) = self.active_tab_position() {
            self.select_tab_row(position);
        } else {
            self.selected = 0;
        }
    }

    fn select_tab_row(&mut self, position: usize) {
        let rows = self.rows();
        self.selected = rows
            .iter()
            .position(|row| matches!(row, Row::Tab { position: row_position, .. } if *row_position == position))
            .unwrap_or(0);
        self.selected_tab_position = Some(position);
    }

    fn active_tab_id(&self) -> Option<usize> {
        self.tabs
            .iter()
            .find(|tab| tab.active)
            .map(|tab| tab.tab_id)
    }

    fn active_tab_position(&self) -> Option<usize> {
        self.tabs
            .iter()
            .find(|tab| tab.active)
            .map(|tab| tab.position)
    }

    fn last_active_tab_position(&self) -> Option<usize> {
        self.last_active_tab_id.and_then(|tab_id| {
            self.tabs
                .iter()
                .find(|tab| tab.tab_id == tab_id)
                .map(|tab| tab.position)
        })
    }

    fn tab_context_for_pane_key(&self, key: &str) -> Option<TabContext> {
        self.panes.iter().find_map(|(position, panes)| {
            let pane = panes.iter().find(|pane| pane_key(&pane_id(pane)) == key)?;
            self.tabs
                .iter()
                .find(|tab| tab.position == *position)
                .map(|tab| TabContext {
                    tab_id: tab.tab_id,
                    focused: pane.is_focused,
                })
        })
    }

    fn first_work_pane_in_tab(&self, position: usize) -> Option<PaneId> {
        self.panes
            .get(&position)
            .and_then(|panes| panes.iter().find(|pane| self.is_work_pane(pane)))
            .map(pane_id)
    }

    fn pane_in_tab(&self, position: usize, pane_id_to_find: &PaneId) -> bool {
        self.panes
            .get(&position)
            .map(|panes| {
                panes
                    .iter()
                    .any(|pane| &pane_id(pane) == pane_id_to_find && self.is_work_pane(pane))
            })
            .unwrap_or(false)
    }

    fn own_pane_in_tab(&self, position: usize) -> bool {
        self.panes
            .get(&position)
            .map(|panes| panes.iter().any(|pane| self.is_own_pane(pane)))
            .unwrap_or(false)
    }

    fn own_plugin_url(&self) -> Option<&str> {
        self.panes
            .values()
            .flatten()
            .find(|pane| self.is_own_pane(pane))
            .and_then(|pane| pane.plugin_url.as_deref())
    }

    fn store_url(&self) -> String {
        let Some(own_url) = self.own_plugin_url() else {
            return DEFAULT_STORE_URL.to_string();
        };
        if own_url.contains("opencode-sidebar") {
            return own_url.replacen("opencode-sidebar", "opencode-sidebar-store", 1);
        }
        if own_url.contains("zellij-agent-session-manager") {
            return own_url.replacen(
                "zellij-agent-session-manager",
                "zellij-agent-session-manager-store",
                1,
            );
        }
        DEFAULT_STORE_URL.to_string()
    }

    fn is_work_pane(&self, pane: &PaneInfo) -> bool {
        !pane.is_plugin && !self.is_own_pane(pane)
    }

    fn is_own_pane(&self, pane: &PaneInfo) -> bool {
        self.own_pane_id == Some(pane_id(pane))
    }

    fn unread_count(&self) -> usize {
        self.alerts.values().map(|alert| alert.total()).sum()
    }

    fn update_own_title_if_needed(&mut self) {
        if let Some(PaneId::Plugin(id)) = self.own_pane_id {
            let unread = self.unread_count();
            let title = if unread > 0 {
                format!("opencode-sidebar !{}", unread)
            } else {
                "opencode-sidebar".to_string()
            };
            if self.last_title.as_deref() != Some(title.as_str()) {
                rename_plugin_pane(id, title.as_str());
                self.last_title = Some(title);
            }
        }
    }
}

fn pane_id(pane: &PaneInfo) -> PaneId {
    if pane.is_plugin {
        PaneId::Plugin(pane.id)
    } else {
        PaneId::Terminal(pane.id)
    }
}

fn pane_key(pane_id: &PaneId) -> String {
    match pane_id {
        PaneId::Terminal(id) => format!("terminal_{}", id),
        PaneId::Plugin(id) => format!("plugin_{}", id),
    }
}

fn normalize_pane_key(value: &str) -> String {
    if value.starts_with("terminal_") || value.starts_with("plugin_") {
        value.to_string()
    } else {
        format!("terminal_{}", value)
    }
}

fn command_display(command: &[String]) -> String {
    command.join(" ").trim().to_string()
}

fn is_shell_command(command: &str) -> bool {
    let first = command.split_whitespace().next().unwrap_or_default();
    let name = first.rsplit('/').next().unwrap_or(first);
    matches!(
        name,
        "sh" | "bash" | "zsh" | "fish" | "dash" | "nu" | "pwsh" | "xonsh"
    )
}

fn fit(text: &str, cols: usize) -> String {
    let trimmed = trim(text, cols);
    format!("{:<width$}", trimmed, width = cols)
}

fn trim(text: &str, cols: usize) -> String {
    if text.chars().count() <= cols {
        return text.to_string();
    }
    if cols <= 1 {
        return text.chars().take(cols).collect();
    }
    let mut out: String = text.chars().take(cols - 1).collect();
    out.push('~');
    out
}

fn bold(text: &str) -> String {
    format!("\u{1b}[1m{}\u{1b}[0m", text)
}
fn dim(text: &str) -> String {
    format!("\u{1b}[2m{}\u{1b}[0m", text)
}

fn icon_style(alert: AlertCounts) -> &'static str {
    if alert.opencode_waiting > 0 {
        "\u{1b}[31;1m"
    } else if alert.opencode_done > 0 {
        "\u{1b}[33;1m"
    } else {
        "\u{1b}[36;1m"
    }
}

fn print_row(parts: Vec<(String, &str)>, cols: usize, selected: bool) {
    let bg = if selected { "\u{1b}[48;5;236m" } else { "" };
    let mut visible_len = 0usize;
    let mut line = String::new();
    line.push_str(bg);
    for (text, style) in parts {
        if visible_len >= cols {
            break;
        }
        let remaining = cols - visible_len;
        let original_len = text.chars().count();
        let text = truncate_with_dots(&text, remaining);
        visible_len += text.chars().count();
        line.push_str(style);
        line.push_str(&text);
        line.push_str("\u{1b}[0m");
        line.push_str(bg);
        if original_len > remaining {
            break;
        }
    }
    if visible_len < cols {
        line.push_str(&" ".repeat(cols - visible_len));
    }
    line.push_str("\u{1b}[0m");
    println!("{}", line);
}

fn truncate_with_dots(text: &str, cols: usize) -> String {
    if text.chars().count() <= cols {
        return text.to_string();
    }
    if cols == 0 {
        return String::new();
    }
    if cols <= 2 {
        return ".".repeat(cols);
    }
    let mut out: String = text.chars().take(cols - 2).collect();
    out.push_str("..");
    out
}
