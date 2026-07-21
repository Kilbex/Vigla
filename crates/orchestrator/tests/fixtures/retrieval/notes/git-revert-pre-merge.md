# Reverting a mission to the pre-merge snapshot tag

Every integration tags `pre-merge/<mission_id>` before the merge
commit lands on `supervisor/main`. `RevertButton` calls
`commands.revertMission(mission_id, cwd)`, which `git reset
--hard` supervisor/main to that tag, then emits one
`MissionReverted` event with the restored SHA + tag name. The
employee branches are untouched and can be re-integrated by hand.
