use super::*;

#[derive(Debug, Clone, Copy)]
enum Column {
    Live(usize),
    Parked(usize),
}

fn holds_content(workspace: &Workspace) -> bool {
    !workspace.is_empty() || workspace.pinned
}

fn empty_at(set: &WorkspaceSet, row: usize) -> bool {
    set.workspaces.get(row).is_some_and(|w| !holds_content(w))
}

// Swaps that move the column shown on set `old[k]` to set `new[k]`
fn column_swaps(old: &[usize], new: &[usize]) -> Vec<(usize, usize)> {
    let mut holder = old.to_vec();
    let mut swaps = Vec::new();
    for k in 0..new.len() {
        let (from, to) = (holder[k], new[k]);
        if from != to {
            if let Some(m) = holder.iter().position(|&h| h == to) {
                holder[m] = from;
            }
            holder[k] = to;
            swaps.push((from, to));
        }
    }
    swaps
}

impl WorkspaceSet {
    fn place_in_row(
        &mut self,
        row: usize,
        live: bool,
        state: &mut WorkspaceUpdateGuard<'_, State>,
    ) {
        let workspace = &mut self.workspaces[row];
        state.move_workspace_to_group(self.group, workspace.handle);
        if live {
            workspace.set_output(&self.output, false);
            workspace.refresh();
            state.remove_workspace_state(&workspace.handle, WState::Hidden);
            if row == self.active {
                state.add_workspace_state(&workspace.handle, WState::Active);
            }
        } else {
            // The output is kept: a parked workspace remembers where it was last shown.
            state.add_workspace_state(&workspace.handle, WState::Hidden);
            state.remove_workspace_state(&workspace.handle, WState::Active);
        }
    }
}

impl Workspaces {
    pub(super) fn unpark_all(
        &mut self,
        workspace_state: &mut WorkspaceUpdateGuard<'_, State>,
        xdg_activation_state: &XdgActivationState,
    ) {
        let Some(target) = self.sets.values_mut().next().or(self.backup_set.as_mut()) else {
            return;
        };
        for mut set in self.parked.drain(..) {
            target.absorb_workspaces(
                std::mem::take(&mut set.workspaces),
                set.active,
                workspace_state,
                xdg_activation_state,
            );
            workspace_state.remove_workspace_group(set.group);
        }
    }

    fn swap_in_row(
        &mut self,
        row: usize,
        a: Column,
        b: Column,
        state: &mut WorkspaceUpdateGuard<'_, State>,
    ) {
        match (a, b) {
            (Column::Live(i), Column::Parked(j)) | (Column::Parked(j), Column::Live(i)) => {
                let live = &mut self.sets[i];
                let parked = &mut self.parked[j];
                std::mem::swap(&mut live.workspaces[row], &mut parked.workspaces[row]);
                live.place_in_row(row, true, state);
                parked.place_in_row(row, false, state);
            }
            (Column::Live(i), Column::Live(j)) if i != j => {
                let [(_, x), (_, y)] = self.sets.get_disjoint_indices_mut([i, j]).unwrap();
                std::mem::swap(&mut x.workspaces[row], &mut y.workspaces[row]);
                x.place_in_row(row, true, state);
                y.place_in_row(row, true, state);
            }
            // same column, or parked <-> parked (never needed)
            _ => {}
        }
    }

    fn grid_ring(&self, row: usize) -> Vec<Column> {
        self.spatial_order()
            .into_iter()
            .map(Column::Live)
            .chain(
                self.parked
                    .iter()
                    .enumerate()
                    .filter(|(_, set)| set.workspaces.get(row).is_some_and(holds_content))
                    .map(|(j, _)| Column::Parked(j)),
            )
            .collect()
    }

    fn spatial_order(&self) -> Vec<usize> {
        let mut live = (0..self.sets.len()).collect::<Vec<_>>();
        live.sort_by_key(|&i| {
            let loc = self.sets[i].output.geometry().loc;
            (loc.x, loc.y)
        });
        live
    }

    pub fn sync_grid_order(&mut self, state: &mut WorkspaceUpdateGuard<'_, State>) {
        let order = self.spatial_order();
        if self.mode == WorkspaceMode::Grid {
            let old = self
                .grid_order
                .iter()
                .filter_map(|o| self.sets.get_index_of(o))
                .collect::<Vec<_>>();
            let new = order
                .iter()
                .copied()
                .filter(|i| self.grid_order.contains(self.sets.get_index(*i).unwrap().0))
                .collect::<Vec<_>>();
            let rows = self.sets.first().map_or(0, |(_, s)| s.workspaces.len());
            for (a, b) in column_swaps(&old, &new) {
                for row in 0..rows {
                    self.swap_in_row(row, Column::Live(a), Column::Live(b), state);
                }
            }
        }
        self.grid_order = order
            .into_iter()
            .map(|i| self.sets.get_index(i).unwrap().0.clone())
            .collect();
    }

    pub fn rotate_row(&mut self, forward: bool, state: &mut WorkspaceUpdateGuard<'_, State>) {
        let Some((_, first)) = self.sets.first() else {
            return;
        };
        if self.mode != WorkspaceMode::Grid {
            return;
        }
        let row = first.active;
        let ring = self.grid_ring(row);
        if ring.len() < 2 {
            return;
        }
        let shown = self
            .sets
            .values()
            .map(|set| set.workspaces[row].handle)
            .collect::<Vec<_>>();
        for k in 1..ring.len() {
            let i = if forward { ring.len() - k } else { k };
            self.swap_in_row(row, ring[0], ring[i], state);
        }
        // No slide across scales: the outgoing workspace is drawn at its new output's scale
        let scale = |output: &Output| output.current_scale().fractional_scale();
        let slide = self
            .sets
            .values()
            .zip(&shown)
            .map(|(set, from)| {
                set.animate()
                    && set.workspaces[row].handle != *from
                    && self
                        .shown_space_for_handle(from)
                        .is_some_and(|w| scale(w.output()) == scale(&set.output))
            })
            .collect::<Vec<_>>();
        let start = Instant::now();
        for ((set, from), slide) in self.sets.values_mut().zip(shown).zip(slide) {
            set.previously_active = slide.then_some((
                row,
                WorkspaceDelta::Rotate {
                    start,
                    forward,
                    from,
                },
            ));
        }
    }

    pub(super) fn fill_empty_columns(&mut self, state: &mut WorkspaceUpdateGuard<'_, State>) {
        let rows = self.sets.first().map_or(0, |(_, s)| s.workspaces.len());
        for row in 0..rows {
            for i in 0..self.sets.len() {
                if !empty_at(&self.sets[i], row) {
                    continue;
                }
                let output = self.sets[i].output.clone();
                if let Some(j) = self.parked.iter().position(|set| {
                    set.workspaces
                        .get(row)
                        .is_some_and(|w| holds_content(w) && w.last_shown_on(&output))
                }) {
                    self.swap_in_row(row, Column::Live(i), Column::Parked(j), state);
                } else if let Some(j) = self.sets.values().position(|set| {
                    set.workspaces.get(row).is_some_and(|w| {
                        w.pinned && w.belongs_to(&output) && !w.belongs_to(&set.output)
                    })
                }) {
                    self.swap_in_row(row, Column::Live(i), Column::Live(j), state);
                }
            }
            let empty = self
                .spatial_order()
                .into_iter()
                .filter(|&i| empty_at(&self.sets[i], row))
                .collect::<Vec<_>>();
            let filled = (0..self.parked.len())
                .filter(|&j| {
                    self.parked[j]
                        .workspaces
                        .get(row)
                        .is_some_and(holds_content)
                })
                .collect::<Vec<_>>();
            for (i, j) in empty.into_iter().zip(filled) {
                self.swap_in_row(row, Column::Live(i), Column::Parked(j), state);
            }
        }
    }

    pub fn rotate_into_view(
        &mut self,
        handle: &WorkspaceHandle,
        output: &Output,
        state: &mut WorkspaceUpdateGuard<'_, State>,
    ) {
        let Some((j, row)) = self.parked.iter().enumerate().find_map(|(j, set)| {
            set.workspaces
                .iter()
                .position(|w| w.handle == *handle)
                .map(|row| (j, row))
        }) else {
            return;
        };
        let Some(i) = self.sets.get_index_of(output) else {
            return;
        };
        for set in self.sets.values_mut() {
            let _ = set.activate(row, WorkspaceDelta::new_shortcut(), state);
        }
        self.swap_in_row(row, Column::Live(i), Column::Parked(j), state);
    }

    pub fn parked_handle_for_surface<S>(&self, surface: &S) -> Option<WorkspaceHandle>
    where
        CosmicSurface: PartialEq<S>,
    {
        self.parked
            .iter()
            .flat_map(|set| set.workspaces.iter())
            .find(|w| {
                w.element_for_surface(surface).is_some()
                    || w.get_fullscreen_surfaces().any(|f| f.surface == *surface)
            })
            .map(|w| w.handle)
    }
}
