//! Runtime crate unit tests, relocated from the former `src/lib.rs` standalone
//! wrapper. Exercises the flat re-exports of `ipe_runtime_rust` (the `[lib]`
//! root at `src/mod.rs`) — `IpeResult`, `IpeMaybe`, and the core kernels.

// ============================================================================
// Tests (re-exported for `cargo test` coverage)
// ============================================================================
mod tests {
    use ipe_runtime_rust::*;

    // IpeResult tests
    #[test]
    fn result_ok() {
        let r: IpeResult<&str, i64> = IpeResult::Ok(42);
        assert!(r.is_ok());
        assert_eq!(r.with_default(0), 42);
    }

    #[test]
    fn result_err() {
        let r: IpeResult<&str, i64> = IpeResult::Err("error");
        assert!(r.is_err());
        assert_eq!(r.with_default(0), 0);
    }

    #[test]
    fn result_map_ok() {
        let r: IpeResult<&str, i64> = IpeResult::Ok(5);
        let mapped = ipe_result_map(r, |x| x * 2);
        assert_eq!(mapped.with_default(0), 10);
    }

    #[test]
    fn result_map_err() {
        let r: IpeResult<&str, i64> = IpeResult::Err("error");
        let mapped: IpeResult<&str, i64> = ipe_result_map(r, |x| x * 2);
        assert!(mapped.is_err());
    }

    #[test]
    fn result_and_then_ok() {
        let r: IpeResult<&str, i64> = IpeResult::Ok(5);
        let chained = ipe_result_and_then(r, |x| IpeResult::Ok(x * 2));
        assert_eq!(chained.with_default(0), 10);
    }

    #[test]
    fn result_and_then_err() {
        let r: IpeResult<&str, i64> = IpeResult::Err("e");
        let chained = ipe_result_and_then(r, |x: i64| IpeResult::Ok(x * 2));
        assert!(chained.is_err());
    }

    // IpeMaybe tests
    #[test]
    fn maybe_just() {
        let m: IpeMaybe<i64> = IpeMaybe::Just(42);
        assert!(m.is_just());
        assert_eq!(m.with_default(0), 42);
    }

    #[test]
    fn maybe_nothing() {
        let m: IpeMaybe<i64> = IpeMaybe::Nothing;
        assert!(m.is_nothing());
        assert_eq!(m.with_default(99), 99);
    }

    #[test]
    fn maybe_map_just() {
        let m: IpeMaybe<i64> = IpeMaybe::Just(5);
        let mapped = ipe_maybe_map(m, |x| x * 2);
        assert_eq!(mapped.with_default(0), 10);
    }

    #[test]
    fn maybe_and_then_just() {
        let m: IpeMaybe<i64> = IpeMaybe::Just(5);
        let chained = ipe_maybe_and_then(m, |x| IpeMaybe::Just(x * 2));
        assert_eq!(chained.with_default(0), 10);
    }

    // List tests — the live, codegen-emitted kernels (list.rs)
    #[test]
    fn list_filter_keeps_matching() {
        assert_eq!(
            list_filter(|x: i64| x % 2 == 0, vec![1, 2, 3, 4]),
            vec![2, 4]
        );
    }

    #[test]
    fn list_foldl_sums() {
        assert_eq!(list_foldl(|x, acc| acc + x, 0, vec![1, 2, 3]), 6);
    }

    #[test]
    fn list_range_inclusive() {
        assert_eq!(list_range(1, 3), vec![1, 2, 3]);
    }

    #[test]
    fn list_member_finds() {
        assert!(list_member(2, vec![1, 2, 3]));
        assert!(!list_member(9, vec![1, 2, 3]));
    }

    #[test]
    fn list_cons_prepends() {
        assert_eq!(ipe_list_cons(0, vec![1, 2]), vec![0, 1, 2]);
    }

    // String tests — the live kernels (string.rs)
    #[test]
    fn string_ops() {
        assert_eq!(string_append("a".into(), "b".into()), "ab");
        assert_eq!(string_length("hello".into()), 5);
        assert!(string_is_empty(String::new()));
    }

    #[test]
    fn string_to_int_ok() {
        assert_eq!(string_to_int("42".into()), IpeMaybe::Just(42));
    }

    #[test]
    fn string_to_int_fail() {
        assert_eq!(string_to_int("abc".into()), IpeMaybe::Nothing);
    }

    // Result helpers
    #[test]
    fn result_with_default_ok() {
        let r: IpeResult<&str, i64> = IpeResult::Ok(42);
        assert_eq!(result_with_default(0, r), 42);
    }

    #[test]
    fn result_traverse_ok() {
        let items = vec![1, 2, 3];
        let r = result_traverse(
            |x: i64| -> IpeResult<&str, i64> { IpeResult::Ok(x * 2) },
            items,
        );
        assert_eq!(r.with_default(vec![]), vec![2, 4, 6]);
    }
}

// ============================================================================
// Tui headless render tests (gated on the `tui` Cargo feature)
// ============================================================================
//
// `tui_app_ui` / `tui_app` open the alternate screen (`TuiGuard::enter_mouse`)
// and therefore require a real TTY — they cannot be invoked from a test.
// These tests exercise the *render half* independently: they build an
// `Element` tree using the same `ipe_runtime::ui::helpers` builders that ipe
// emits, call `tui::layout::element_to_cells` (headless — no TTY), and assert
// the resulting ANSI-cell frame string contains the expected content.
//
// This mirrors the Ipê counter's `view { count = 0 }` call at initial state:
//
//   view model =
//     Ui.column [] [ Ui.el [] (Ui.text (String.fromInt model.count)) ]
//
// with `model.count = 0`, so `String.fromInt 0 = "0"`.
#[cfg(all(test, feature = "tui"))]
mod tui_headless {
    use ipe_runtime_rust::tui::layout::element_to_cells;
    use ipe_runtime_rust::ui::Attribute;
    use ipe_runtime_rust::ui::helpers::{ui_column_, ui_el_, ui_text_};

    /// Render a `Ui.column [] [ Ui.el [] (Ui.text "0") ]` tree to a headless
    /// 80×24 ANSI cell frame and verify it contains the digit `"0"`.
    ///
    /// This is the render half of the Tui golden.  The build half (ipe +
    /// cargo build the full Tui counter program) lives in
    /// `crates/ipe/tests/tui_e2e.rs::tui_counter_build_only`.
    #[test]
    fn tui_headless_render_contains_count() {
        // Construct the element tree equivalent to:
        //   view { count = 0 } =
        //     Ui.column [] [ Ui.el [] (Ui.text "0") ]
        //
        // `()` is the message type — irrelevant for a pure render, no events fired.
        let elem = ui_column_::<()>(
            Vec::<Attribute<()>>::new(),
            vec![ui_el_::<()>(
                Vec::<Attribute<()>>::new(),
                ui_text_::<()>("0".to_string()),
            )],
        );

        // Render headless (no TTY required).  80 columns × 24 rows = standard
        // terminal size.
        let frame = element_to_cells(&elem, 80, 24);

        // The frame MUST contain "0" — the counter value at initial state.
        assert!(
            frame.contains('0'),
            "expected rendered frame to contain '0', got:\n{frame}"
        );
    }
}
