// Cmd-wiring emit composition is exercised end to end in
// [`crate::emit_web`]'s `hot_appearance_tests` (a `Cmd.perform` arm composes the
// `fire_cmd_wiring` dispatch; a `Cmd.none` arm and the flag-off case do not) and
// the wiring VOCABULARY in [`crate::transition_classify`]'s `tests`, so no
// standalone emit test lives here.
