docs: handoff reflects working from inside the work box

The agent runs as dev inside the work box, so drop the container exec wrappers and use
plain worktree paths/commands. Note that the host and the Bifrost host aren't
reachable from here and list the host-only pieces (deploy scripts, systemd
units, env, host.yaml, schedulers) to request from the user instead. Also
note the shared stash stack and the nobody-owned-file bind-mount quirk.
