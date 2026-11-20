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
            // live <-> live and parked <-> parked are never needed
            _ => {}
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
                }
            }
        }
    }
}
