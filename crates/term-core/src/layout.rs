use serde::{Deserialize, Serialize};

pub type PaneId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LayoutNode {
    Pane(PaneId),
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub id: u64,
    pub name: Option<String>,
    pub layout: LayoutNode,
    pub focused_pane: PaneId,
    pub zoomed: bool,
}

impl Tab {
    pub fn new(id: u64, first_pane: PaneId) -> Self {
        Self {
            id,
            name: None,
            layout: LayoutNode::Pane(first_pane),
            focused_pane: first_pane,
            zoomed: false,
        }
    }

    pub fn split_focused(&mut self, new_pane: PaneId, direction: SplitDirection) -> bool {
        if self
            .layout
            .split_pane(self.focused_pane, new_pane, direction)
        {
            self.focused_pane = new_pane;
            self.zoomed = false;
            true
        } else {
            false
        }
    }

    pub fn can_goto_split(&self, dir: NavDirection, area: Rect) -> bool {
        self.layout
            .find_neighbor(self.focused_pane, dir, area)
            .is_some()
    }

    /// `split-preserve-zoom = navigation` 対応: zoomed 状態を保ったまま移動先ペインへフォーカスを移す。
    pub fn goto_split(&mut self, dir: NavDirection, area: Rect) -> bool {
        if let Some(next) = self.layout.find_neighbor(self.focused_pane, dir, area) {
            self.focused_pane = next;
            true
        } else {
            false
        }
    }

    pub fn toggle_zoom(&mut self) {
        self.zoomed = !self.zoomed;
    }

    pub fn resize_focused(&mut self, dir: NavDirection, step: f32) -> bool {
        if self.zoomed {
            return false;
        }
        self.layout.resize_pane(self.focused_pane, dir, step)
    }

    pub fn visible_rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        if self.zoomed {
            vec![(self.focused_pane, area)]
        } else {
            self.layout.compute_rects(area)
        }
    }

    pub fn contains_pane(&self, target: PaneId) -> bool {
        self.layout.contains_pane(target)
    }

    pub fn all_panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.layout.collect_panes(&mut out);
        out
    }

    /// 指定ペインを閉じる。タブ内に残りペインがあれば `true`、最後の1枚だったら `false` を返す。
    pub fn close_pane(&mut self, target: PaneId) -> bool {
        if matches!(self.layout, LayoutNode::Pane(id) if id == target) {
            return false;
        }
        if self.layout.remove_pane(target) && self.focused_pane == target {
            self.focused_pane = self.layout.first_pane();
            self.zoomed = false;
        }
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}

impl Workspace {
    pub fn pane_count(&self) -> usize {
        self.tabs.iter().map(|t| t.all_panes().len()).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceManager {
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
}

impl WorkspaceManager {
    pub fn new(first_pane: PaneId) -> Self {
        Self {
            workspaces: vec![Workspace {
                id: "master".to_string(),
                name: "Master Space".to_string(),
                tabs: vec![Tab::new(1, first_pane)],
                active_tab: 0,
            }],
            active_workspace: 0,
        }
    }

    pub fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active_workspace]
    }

    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_workspace]
    }

    pub fn active_tab(&self) -> &Tab {
        let ws = self.active_workspace();
        &ws.tabs[ws.active_tab]
    }

    pub fn active_tab_mut(&mut self) -> &mut Tab {
        let ws = self.active_workspace_mut();
        &mut ws.tabs[ws.active_tab]
    }

    pub fn add_workspace(&mut self, name: String, first_pane: PaneId) -> usize {
        let idx = self.workspaces.len();
        self.workspaces.push(Workspace {
            id: format!("ws-{idx}"),
            name,
            tabs: vec![Tab::new(1, first_pane)],
            active_tab: 0,
        });
        self.active_workspace = idx;
        idx
    }

    pub fn add_tab(&mut self, first_pane: PaneId) -> usize {
        let ws = self.active_workspace_mut();
        let next_id = ws.tabs.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        ws.tabs.push(Tab::new(next_id, first_pane));
        ws.active_tab = ws.tabs.len() - 1;
        ws.active_tab
    }

    pub fn switch_workspace(&mut self, idx: usize) {
        if idx < self.workspaces.len() {
            self.active_workspace = idx;
        }
    }

    pub fn rename_workspace(&mut self, idx: usize, new_name: String) -> bool {
        let trimmed = new_name.trim();
        if idx < self.workspaces.len() && !trimmed.is_empty() {
            self.workspaces[idx].name = trimmed.to_string();
            true
        } else {
            false
        }
    }

    /// 指定 Workspace を閉じて配下の全 PaneId を返す（index 0 の Master Space は保護する）。
    pub fn close_workspace(&mut self, idx: usize) -> Vec<PaneId> {
        if idx == 0 || idx >= self.workspaces.len() {
            return Vec::new();
        }
        let removed = self.workspaces.remove(idx);
        let mut panes = Vec::new();
        for tab in &removed.tabs {
            panes.extend(tab.all_panes());
        }
        if self.active_workspace >= self.workspaces.len() {
            self.active_workspace = self.workspaces.len() - 1;
        } else if self.active_workspace > idx {
            self.active_workspace -= 1;
        }
        panes
    }

    pub fn switch_tab(&mut self, idx: usize) {
        let ws = self.active_workspace_mut();
        if idx < ws.tabs.len() {
            ws.active_tab = idx;
        }
    }

    /// 指定インデックスのタブを閉じる。閉じたタブ内の全 PaneId と、アプリを継続するか (`true`) を返す。
    pub fn close_tab(&mut self, tab_idx: usize) -> (Vec<PaneId>, bool) {
        let ws_idx = self.active_workspace;
        if tab_idx >= self.workspaces[ws_idx].tabs.len() {
            return (Vec::new(), true);
        }
        let removed = self.workspaces[ws_idx].tabs.remove(tab_idx);
        let panes = removed.all_panes();

        if self.workspaces[ws_idx].tabs.is_empty() {
            if self.workspaces.len() == 1 {
                return (panes, false);
            }
            self.workspaces.remove(ws_idx);
            if self.active_workspace >= self.workspaces.len() {
                self.active_workspace = self.workspaces.len() - 1;
            }
        } else if self.workspaces[ws_idx].active_tab >= self.workspaces[ws_idx].tabs.len() {
            self.workspaces[ws_idx].active_tab = self.workspaces[ws_idx].tabs.len() - 1;
        }
        (panes, true)
    }

    /// 指定ペインが属する Workspace/Tab からペインを閉じる。
    /// アプリ全体で残りペインが無くなったら `false` を返す。
    pub fn close_pane(&mut self, target: PaneId) -> bool {
        for ws_idx in 0..self.workspaces.len() {
            let mut empty_tab_idx = None;
            for tab_idx in 0..self.workspaces[ws_idx].tabs.len() {
                let tab = &mut self.workspaces[ws_idx].tabs[tab_idx];
                if tab.contains_pane(target) {
                    if !tab.close_pane(target) {
                        empty_tab_idx = Some(tab_idx);
                    }
                    break;
                }
            }
            if let Some(t_idx) = empty_tab_idx {
                self.workspaces[ws_idx].tabs.remove(t_idx);
                if self.workspaces[ws_idx].tabs.is_empty() {
                    if self.workspaces.len() == 1 {
                        return false;
                    }
                    self.workspaces.remove(ws_idx);
                    if self.active_workspace >= self.workspaces.len() {
                        self.active_workspace = self.workspaces.len() - 1;
                    }
                } else if self.workspaces[ws_idx].active_tab >= self.workspaces[ws_idx].tabs.len() {
                    self.workspaces[ws_idx].active_tab = self.workspaces[ws_idx].tabs.len() - 1;
                }
                return true;
            }
        }
        true
    }
}

impl LayoutNode {
    /// 指定されたペインを分割し、新しいペインIDをレイアウト木へ組み込む。
    pub fn split_pane(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        direction: SplitDirection,
    ) -> bool {
        match self {
            LayoutNode::Pane(id) if *id == target => {
                let old = Box::new(LayoutNode::Pane(target));
                let new = Box::new(LayoutNode::Pane(new_pane));
                *self = LayoutNode::Split {
                    direction,
                    ratio: 0.5,
                    first: old,
                    second: new,
                };
                true
            }
            LayoutNode::Pane(_) => false,
            LayoutNode::Split { first, second, .. } => {
                first.split_pane(target, new_pane, direction)
                    || second.split_pane(target, new_pane, direction)
            }
        }
    }

    pub fn first_pane(&self) -> PaneId {
        match self {
            LayoutNode::Pane(id) => *id,
            LayoutNode::Split { first, .. } => first.first_pane(),
        }
    }

    pub fn contains_pane(&self, target: PaneId) -> bool {
        match self {
            LayoutNode::Pane(id) => *id == target,
            LayoutNode::Split { first, second, .. } => {
                first.contains_pane(target) || second.contains_pane(target)
            }
        }
    }

    pub fn collect_panes(&self, out: &mut Vec<PaneId>) {
        match self {
            LayoutNode::Pane(id) => out.push(*id),
            LayoutNode::Split { first, second, .. } => {
                first.collect_panes(out);
                second.collect_panes(out);
            }
        }
    }

    /// 分割木から `target` を取り除き、相方のノードを親の位置へ繰り上げる。
    pub fn remove_pane(&mut self, target: PaneId) -> bool {
        match self {
            LayoutNode::Pane(_) => false,
            LayoutNode::Split { first, second, .. } => {
                if matches!(**first, LayoutNode::Pane(id) if id == target) {
                    let survivor = std::mem::replace(second.as_mut(), LayoutNode::Pane(0));
                    *self = survivor;
                    return true;
                }
                if matches!(**second, LayoutNode::Pane(id) if id == target) {
                    let survivor = std::mem::replace(first.as_mut(), LayoutNode::Pane(0));
                    *self = survivor;
                    return true;
                }
                first.remove_pane(target) || second.remove_pane(target)
            }
        }
    }

    /// 矩形領域を再帰的に分割し、各ペインの描画矩形を返す。
    pub fn compute_rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.collect_rects(area, &mut out);
        out
    }

    fn collect_rects(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            LayoutNode::Pane(id) => out.push((*id, area)),
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => match direction {
                SplitDirection::Horizontal => {
                    let w1 = area.width * *ratio;
                    let w2 = area.width - w1;
                    first.collect_rects(Rect { width: w1, ..area }, out);
                    second.collect_rects(
                        Rect {
                            x: area.x + w1,
                            width: w2,
                            ..area
                        },
                        out,
                    );
                }
                SplitDirection::Vertical => {
                    let h1 = area.height * *ratio;
                    let h2 = area.height - h1;
                    first.collect_rects(Rect { height: h1, ..area }, out);
                    second.collect_rects(
                        Rect {
                            y: area.y + h1,
                            height: h2,
                            ..area
                        },
                        out,
                    );
                }
            },
        }
    }

    /// `performable:goto_split` 用: 隣接するペインが存在する場合のみその PaneId を返す。
    pub fn find_neighbor(&self, current: PaneId, dir: NavDirection, area: Rect) -> Option<PaneId> {
        let rects = self.compute_rects(area);
        let (_, cur_rect) = rects.iter().find(|(id, _)| *id == current)?;
        let cur_cx = cur_rect.x + cur_rect.width * 0.5;
        let cur_cy = cur_rect.y + cur_rect.height * 0.5;

        rects
            .iter()
            .filter(|(id, _)| *id != current)
            .filter(|(_, r)| match dir {
                NavDirection::Left => r.x + r.width <= cur_rect.x + 1.0,
                NavDirection::Right => r.x >= cur_rect.x + cur_rect.width - 1.0,
                NavDirection::Up => r.y + r.height <= cur_rect.y + 1.0,
                NavDirection::Down => r.y >= cur_rect.y + cur_rect.height - 1.0,
            })
            .min_by(|(_, a), (_, b)| {
                let da = (a.x + a.width * 0.5 - cur_cx).hypot(a.y + a.height * 0.5 - cur_cy);
                let db = (b.x + b.width * 0.5 - cur_cx).hypot(b.y + b.height * 0.5 - cur_cy);
                da.total_cmp(&db)
            })
            .map(|(id, _)| *id)
    }

    pub fn resize_pane(&mut self, target: PaneId, dir: NavDirection, step: f32) -> bool {
        let want_split = match dir {
            NavDirection::Left | NavDirection::Right => SplitDirection::Horizontal,
            NavDirection::Up | NavDirection::Down => SplitDirection::Vertical,
        };
        let delta = match dir {
            NavDirection::Left | NavDirection::Up => -step.abs(),
            NavDirection::Right | NavDirection::Down => step.abs(),
        };
        match self {
            Self::Pane(_) => false,
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                if first.contains_pane(target) && first.resize_pane(target, dir, step) {
                    return true;
                }
                if second.contains_pane(target) && second.resize_pane(target, dir, step) {
                    return true;
                }
                if *direction == want_split
                    && (first.contains_pane(target) || second.contains_pane(target))
                {
                    *ratio = (*ratio + delta).clamp(0.1, 0.9);
                    return true;
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_find_neighbor() {
        let mut root = LayoutNode::Pane(1);
        assert!(root.split_pane(1, 2, SplitDirection::Horizontal));
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 600.0,
        };
        assert_eq!(root.find_neighbor(1, NavDirection::Right, area), Some(2));
        assert_eq!(root.find_neighbor(1, NavDirection::Left, area), None);
        assert_eq!(root.find_neighbor(2, NavDirection::Left, area), Some(1));
    }

    #[test]
    fn close_pane_collapses_split_and_updates_focus() {
        let mut tab = Tab::new(1, 1);
        assert!(tab.split_focused(2, SplitDirection::Horizontal));
        assert_eq!(tab.focused_pane, 2);

        // ペイン2を閉じるとペイン1が残りフォーカスが移る
        assert!(tab.close_pane(2));
        assert_eq!(tab.focused_pane, 1);

        // 最後のペイン1を閉じると false（タブ終了）
        assert!(!tab.close_pane(1));
    }

    #[test]
    fn workspace_manager_creates_master_and_dev_spaces() {
        let mut mgr = WorkspaceManager::new(1);
        assert_eq!(mgr.workspaces[0].name, "Master Space");
        let idx = mgr.add_workspace("conductty".to_string(), 2);
        assert_eq!(idx, 1);
        assert_eq!(mgr.active_workspace().name, "conductty");

        // Dev Workspace の最後のペインを閉じると Master Space に戻る
        assert!(mgr.close_pane(2));
        assert_eq!(mgr.workspaces.len(), 1);
        assert_eq!(mgr.active_workspace().name, "Master Space");
    }

    #[test]
    fn close_tab_returns_panes_and_updates_active_tab() {
        let mut mgr = WorkspaceManager::new(1);
        mgr.add_tab(2);
        assert_eq!(mgr.active_workspace().tabs.len(), 2);
        assert_eq!(mgr.active_workspace().active_tab, 1);

        let (closed_panes, keep_running) = mgr.close_tab(1);
        assert!(keep_running);
        assert_eq!(closed_panes, vec![2]);
        assert_eq!(mgr.active_workspace().tabs.len(), 1);
        assert_eq!(mgr.active_workspace().active_tab, 0);
    }

    #[test]
    fn rename_and_close_workspace_protects_master_space() {
        let mut mgr = WorkspaceManager::new(1);
        let idx = mgr.add_workspace("Dev 1".to_string(), 2);
        assert!(mgr.rename_workspace(idx, "omni-desk".to_string()));
        assert_eq!(mgr.workspaces[idx].name, "omni-desk");

        // Master Space (index 0) は削除されない
        assert!(mgr.close_workspace(0).is_empty());
        assert_eq!(mgr.workspaces.len(), 2);

        // Dev Workspace を閉じると配下のペインIDが返る
        let closed = mgr.close_workspace(idx);
        assert_eq!(closed, vec![2]);
        assert_eq!(mgr.workspaces.len(), 1);
        assert_eq!(mgr.active_workspace, 0);
    }

    #[test]
    fn resize_focused_adjusts_split_ratio_and_clamps() {
        let mut tab = Tab::new(1, 1);
        assert!(tab.split_focused(2, SplitDirection::Horizontal));
        assert!(tab.resize_focused(NavDirection::Left, 0.1));
        let rects = tab.visible_rects(Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        });
        assert!((rects[0].1.width - 40.0).abs() < 0.01);
        assert!((rects[1].1.width - 60.0).abs() < 0.01);

        // 極端に縮めても 0.1..=0.9 でクランプされる
        for _ in 0..20 {
            tab.resize_focused(NavDirection::Right, 0.1);
        }
        let rects_clamped = tab.visible_rects(Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
        });
        assert!((rects_clamped[0].1.width - 90.0).abs() < 0.01);
    }
}
