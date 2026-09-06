#[cfg(test)]
mod cmd_wiring_emit_tests {
    use super::emit_cmd_wiring_arm;
    use ipe_ir::{CallPin, Callee, Expr, KernelFn, OnFormKind};

    fn cmd_none() -> Expr {
        Expr::Call {
            callee: Callee::Kernel(KernelFn::CmdNone),
            args: vec![],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    // A `Cmd.none` arm classifies as the no-effect wiring and emits the compiled
    // `select_cmd_hot` selector over the arm's effect table (here empty).
    #[test]
    fn cmd_none_arm_emits_no_effect_selector() {
        // `( <model>, Cmd.none )` — the model half is irrelevant to the wiring.
        let body = Expr::Tuple(vec![Expr::Int(0), cmd_none()]);
        assert_eq!(
            emit_cmd_wiring_arm(&body, 0),
            Some(r#"ipe_runtime::web::select_cmd_hot("{\"effect\":null}", 0)"#.to_owned()),
            "a Cmd.none arm emits the no-effect wiring selector"
        );
    }

    // A real effect body (a non-`Cmd.none` second element) is not a recognised
    // wiring — the arm's Cmd stays compiled (`None`).
    #[test]
    fn real_cmd_arm_emits_nothing() {
        let real = Expr::Call {
            callee: Callee::Kernel(KernelFn::CmdBatch),
            args: vec![],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        let body = Expr::Tuple(vec![Expr::Int(0), real]);
        assert_eq!(emit_cmd_wiring_arm(&body, 2), None);
    }
}
