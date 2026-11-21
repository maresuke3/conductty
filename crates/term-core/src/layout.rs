use serde::{Deserialize, Serialize};

pub type PaneId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        if self.layout.split_pane(self.focused_pane, new_pane, direction) {
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

    pub fn visible_rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        if self.zoomed {
            vec![(self.focused_pane, area)]
        } else {
            self.layout.compute_rects(area)
        }
    }

    /// 指定ペインを閉じる。タブ内に残りペインがあれば `true`、最後の1枚だったら `false` を返す。
    pub fn close_pane(&mut self, target: PaneId) -> bool {
        if matches!(self.layout, LayoutNode::Pane(id) if id == target) {
            return false;
        }
        if self.layout.remove_pane(target) {
            if self.focused_pane == target {
                self.focused_pane = self.layout.first_pane();
                self.zoomed = false;
            }
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
                    first.collect_rects(
                        Rect {
                            width: w1,
                            ..area
                        },
                        out,
                    );
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
                    first.collect_rects(
                        Rect {
                            height: h1,
                            ..area
                        },
                        out,
                    );
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
}
