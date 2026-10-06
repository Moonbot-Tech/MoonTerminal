//! Strategy tree built on MoonUI's headless `Tree::custom` mode.
//! MoonTree flattens `expanded_ids`, virtualizes rows, handles keyboard input, and supplies row
//! hitboxes to DnD decorators. Selection, staging, and expansion remain in `StrategiesView`; this
//! module adapts `CoreStore` to `MoonTreeItem`, builds the `id -> NodeData` side map for row and drag
//! data, and renders rows and decorators. Callbacks outside `Context<Self>` mutate through
//! `Entity::update`.

/// MoonTree build helpers.
mod build;
/// MoonTree callbacks helpers.
mod callbacks;
/// MoonTree counts helpers.
mod counts;
/// MoonTree geometry helpers.
mod geometry;
/// MoonTree headings helpers.
mod headings;
/// MoonTree node helpers.
mod node;
/// MoonTree rows helpers.
mod rows;
/// MoonTree shape helpers.
mod shape;
/// MoonTree strategy row helpers.
mod strategy_row;

pub(crate) use build::build;
pub(super) use callbacks::drop_dest;
use callbacks::*;
pub(crate) use counts::FolderFill;
use counts::*;
use geometry::*;
pub(crate) use geometry::{id_del_strat, id_strat};
use headings::*;
pub(crate) use node::{MoonTreeBuild, NodeData};
use rows::*;
pub(crate) use shape::shape_sig;
use shape::*;
use strategy_row::*;

use moon_core::feed::strategy_path;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonDisclosure, MoonPalette, MoonText, MoonTheme,
    MoonTone, MoonTree, MoonTreeEntry, MoonTreeItem, MoonTreeRowMeta, h_flex,
};

use super::super::filter::PreparedFilter;
use super::super::logic::{
    FolderCounts, build_node, ensure_folder, strategy_core_is_visible, subtree_check_targets,
    subtree_displayed_all_checked, toggle,
};
use super::super::{Key, StrategiesView, moon_alpha};
use super::checks;
use super::ops;
use super::ui::{ContextMenu, DragChip, FolderDrag, MenuTarget, StratDrag};
use crate::controls::core_run::{RunKey, RunScope, RunSlots, reserved_cell, run_cell};
use crate::design;
use moon_core::feed::StrategyRow;
use moon_core::session::{CoreId, CoreStore};
use moon_core::venue::CoreVenue;

#[cfg(test)]
mod tests;
