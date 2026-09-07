use super::{
    BTreeMap, Builder, BuiltinTag, FieldTag, RowTail, RowTailShape, SchemeKey, SchemeSlot,
    StdlibKernel, Symbol, Ty, TyShape,
};

impl Builder<'_> {
    /// Resolve a [`SchemeKey`] carried on a [`ipe_kernels::KernelDef`] to its
    /// concrete HM type scheme.
    ///
    /// A [`SchemeKey`] names a kernel's scheme without carrying it (the scheme is
    /// built from interned `Symbol`s that exist only after the `Interner` runs,
    /// so it cannot be a `'static` value on the descriptor). This is the single
    /// interpreter that turns the key back into a `Ty`: it delegates to
    /// [`Self::stdlib_scheme`], the authoritative scheme table, keyed on the
    /// kernel identity the key wraps. `None` mirrors `stdlib_scheme` — the kernel
    /// has no registry scheme (a routed / unlowered bucket). Routing every
    /// `def().scheme` read through this one adapter means the descriptor's scheme
    /// reference and the table can never resolve to different types.
    pub fn resolve_scheme(&self, key: SchemeKey) -> Option<Ty> {
        // Memoised per kernel: a kernel's scheme depends only on the interned
        // built-in names, fixed for the builder's lifetime, so it is built at
        // most once and cloned thereafter. A cached value is byte-identical to a
        // rebuild by construction (same pure inputs); `instantiate_in` still
        // alpha-renames per use site, so instantiation is unaffected.
        let idx = key.0 as usize;
        if let Some(SchemeSlot::Resolved(cached)) = self.scheme_cache.borrow().get(idx) {
            return cached.clone();
        }
        // A kernel that carries a structural `TyShape` is resolved by
        // interpreting it; the result is byte-identical to the `stdlib_scheme`
        // table's (pinned by `interpreted_shape_matches_legacy`). One without a
        // shape (`shape == None`) resolves through the table.
        let resolved = key.0.def().shape.map_or_else(
            || self.stdlib_scheme(key.0),
            |shape| Some(self.interpret_shape(shape)),
        );
        if let Some(slot) = self.scheme_cache.borrow_mut().get_mut(idx) {
            *slot = SchemeSlot::Resolved(resolved.clone());
        }
        resolved
    }

    /// Interpret a `'static` [`TyShape`] into a concrete [`Ty`], resolving each
    /// [`BuiltinTag`] against the interned-symbol cache.
    ///
    /// The single interpreter a structural kernel scheme routes through. Its
    /// output is byte-identical to the `Ty` [`Self::stdlib_scheme`] produces for
    /// the same kernel (proven per-kernel by the `interpreted_shape_matches_legacy`
    /// tripwire).
    ///
    /// It touches no union-find state even for the polymorphic [`TyShape::Var`]
    /// node: a scheme var is interpreted to the SAME placeholder `Ty::Var` the
    /// `stdlib_scheme` table's `var(i)` builder produces — the bare positional
    /// index raw, in annotation-symbol space — NOT a fresh union-find var.
    /// Generalization / instantiation with fresh solver vars happens later at the
    /// use site (`instantiate_in`), exactly as for a table-built scheme, so this
    /// interpreter still takes `&self`. Because `Ty::Var` is `Eq`, repeating an
    /// index reuses one variable structurally without any shared-cell handling.
    pub fn interpret_shape(&self, shape: &TyShape) -> Ty {
        match shape {
            TyShape::Fun(arg, res) => Ty::Fun(
                Box::new(self.interpret_shape(arg)),
                Box::new(self.interpret_shape(res)),
            ),
            TyShape::Con(tag, args) => Ty::Con {
                module: self.builtin_con_module(*tag),
                name: self.builtin_symbol(*tag),
                args: args.iter().map(|a| self.interpret_shape(a)).collect(),
            },
            // Element order is preserved, matching the hand-built
            // `Ty::Tuple(vec![…])` a `stdlib_scheme` arm produces.
            TyShape::Tuple(elems) => {
                Ty::Tuple(elems.iter().map(|e| self.interpret_shape(e)).collect())
            }
            // The `BTreeMap` re-sorts by the resolved field `Symbol`, so the
            // key order is byte-identical to the hand-built `Ty::Record`
            // regardless of the declared slice order (the declared order is
            // additionally pinned ascending by `interpreted_shape_matches_legacy`).
            TyShape::Record { fields, tail } => {
                let mut map = BTreeMap::new();
                for (name, field) in *fields {
                    map.insert(self.field_symbol(*name), self.interpret_shape(field));
                }
                let tail = match tail {
                    RowTailShape::Closed => RowTail::Closed,
                    RowTailShape::Open(i) => RowTail::Open(u32::from(*i)),
                };
                Ty::Record(map, tail)
            }
            // The `stdlib_scheme` table binds `let var = Ty::Var`, so its
            // `var(i)` is `Ty::Var(i)`: a scheme-local variable's raw is its bare
            // positional index. Match that exactly for byte-identity.
            TyShape::Var(i) => Ty::Var(u32::from(*i)),
            // The `stdlib_scheme` table materialises `()` as the bare `Ty::Unit`
            // leaf; match it exactly.
            TyShape::Unit => Ty::Unit,
        }
    }

    /// Resolve a structural [`BuiltinTag`] to the interned type-constructor
    /// [`Symbol`] the `stdlib_scheme` table uses for the same built-in, so an
    /// interpreted shape is byte-identical to the hand-built `Ty`.
    pub const fn builtin_symbol(&self, tag: BuiltinTag) -> Symbol {
        match tag {
            BuiltinTag::Int => self.builtins.int,
            BuiltinTag::Float => self.builtins.float,
            BuiltinTag::Bool => self.builtins.bool,
            BuiltinTag::String => self.builtins.string,
            BuiltinTag::Char => self.builtins.char,
            BuiltinTag::Bytes => self.builtins.bytes,
            BuiltinTag::List => self.builtins.list,
            BuiltinTag::Maybe => self.builtins.maybe,
            BuiltinTag::Result => self.builtins.result,
            BuiltinTag::Set => self.builtins.set,
            BuiltinTag::Dict => self.builtins.dict,
            BuiltinTag::Order => self.builtins.order,
            BuiltinTag::Error => self.builtins.error,
            BuiltinTag::ErrorKind => self.builtins.errorkind,
            BuiltinTag::ErrorDetails => self.builtins.errordetails,
            BuiltinTag::Decimal => self.builtins.decimal,
            BuiltinTag::Task => self.builtins.task,
            BuiltinTag::Cmd => self.builtins.cmd,
            BuiltinTag::Sub => self.builtins.sub,
            BuiltinTag::Topic => self.builtins.topic_con,
            BuiltinTag::Decoder => self.builtins.decoder,
            BuiltinTag::Db => self.builtins.db,
            BuiltinTag::SqlValue => self.builtins.sqlvalue,
            BuiltinTag::SqlField => self.builtins.sqlfield,
            BuiltinTag::SqlFragment => self.builtins.sqlfragment,
            BuiltinTag::ProjectionTerm => self.builtins.projection_term,
            BuiltinTag::ProjectionOperand => self.builtins.projection_operand,
            BuiltinTag::Secret => self.builtins.secret,
            BuiltinTag::Path => self.builtins.path,
            BuiltinTag::Regex => self.builtins.regex,
            BuiltinTag::Url => self.builtins.url,
            BuiltinTag::Dsn => self.builtins.dsn,
            BuiltinTag::Connection => self.builtins.connection,
            BuiltinTag::ConnReadOnly => self.builtins.conn_read_only,
            BuiltinTag::ConnReadWrite => self.builtins.conn_read_write,
            BuiltinTag::Setting => self.builtins.setting,
            BuiltinTag::ShapeWeb => self.builtins.shape_web,
            BuiltinTag::ShapeWebView => self.builtins.shape_webview,
            BuiltinTag::ShapeTerminal => self.builtins.shape_terminal,
            BuiltinTag::HostMode => self.builtins.host_mode,
            BuiltinTag::LogLevel => self.builtins.log_level,
            BuiltinTag::CsrfMode => self.builtins.csrf_mode,
            BuiltinTag::RevocationMode => self.builtins.revocation_mode,
            BuiltinTag::Locale => self.builtins.locale,
            BuiltinTag::HttpMethod => self.builtins.http_method,
            BuiltinTag::RedirectPolicy => self.builtins.redirect_policy,
            BuiltinTag::CryptoKey => self.builtins.crypto_key,
            BuiltinTag::CryptoMac => self.builtins.crypto_mac,
            BuiltinTag::EmailAddress => self.builtins.email_address,
            BuiltinTag::Principal => self.builtins.principal,
            BuiltinTag::Claims => self.builtins.jwt_claims,
            BuiltinTag::Algorithm => self.builtins.jwt_algorithm,
            BuiltinTag::JsonValue => self.builtins.json_value,
            BuiltinTag::StreamId => self.builtins.stream_id,
            BuiltinTag::StreamWriter => self.builtins.stream_writer,
            BuiltinTag::WsServer => self.builtins.ws_server,
            BuiltinTag::WsServerCfg => self.builtins.ws_server_cfg,
            BuiltinTag::ServerRequest => self.builtins.server_request,
            BuiltinTag::ServerCookie => self.builtins.server_cookie,
            BuiltinTag::ServerRoute => self.builtins.server_route,
            BuiltinTag::AuthConfig => self.builtins.auth_config,
            BuiltinTag::TokenSource => self.builtins.token_source,
            // `Ipe.Ui.Attribute` and `Ipe.Html.Attribute` share this interned
            // `Attribute` name; they differ only in the module path
            // (`builtin_con_module`).
            BuiltinTag::UiAttribute | BuiltinTag::HtmlAttribute => self.builtins.attribute,
            BuiltinTag::UiElement => self.builtins.element,
            BuiltinTag::Cells => self.builtins.cells,
            BuiltinTag::TuiAttr => self.builtins.tui_attr,
            BuiltinTag::CliLines => self.builtins.cli_lines,
            BuiltinTag::CliAttr => self.builtins.cli_attr,
            BuiltinTag::TermColor => self.builtins.term_color,
            BuiltinTag::CustomElement => self.builtins.custom_element,
            BuiltinTag::Html => self.builtins.html_con,
            BuiltinTag::UiLength => self.builtins.length,
            BuiltinTag::UiColor => self.builtins.color,
            BuiltinTag::UiDescription => self.builtins.description,
            BuiltinTag::UiPseudoClass => self.builtins.pseudo_class,
            BuiltinTag::InputLabel => self.builtins.input_label_con,
            BuiltinTag::InputPlaceholder => self.builtins.input_placeholder_con,
            BuiltinTag::InputRadioOption => self.builtins.input_radio_option_con,
            BuiltinTag::WebReq => self.builtins.web_req,
            BuiltinTag::SessionHandle => self.builtins.session_handle,
            BuiltinTag::WebRoute => self.builtins.live_route_con,
            BuiltinTag::EmailProvider => self.builtins.email_provider,
            BuiltinTag::BackoffStrategy => self.builtins.backoffstrategy,
            BuiltinTag::WebApp => self.builtins.web_app,
            BuiltinTag::TuiApp => self.builtins.tui_app,
            BuiltinTag::CliApp => self.builtins.cli_app,
        }
    }

    /// Resolve a structural [`FieldTag`] to the interned field-name [`Symbol`]
    /// the `stdlib_scheme` table uses as the `Ty::Record` `BTreeMap` key for the
    /// same field, so an interpreted record shape is byte-identical to the
    /// hand-built `Ty::Record`.
    pub const fn field_symbol(&self, tag: FieldTag) -> Symbol {
        match tag {
            FieldTag::MigrationName => self.builtins.migration_f_name,
            FieldTag::MigrationSql => self.builtins.migration_f_sql,
            FieldTag::HttpBody => self.builtins.http_f_body,
            FieldTag::HttpHeaders => self.builtins.http_f_headers,
            FieldTag::HttpStatus => self.builtins.http_f_status,
            FieldTag::HttpMethod => self.builtins.http_f_method,
            FieldTag::HttpUrl => self.builtins.http_f_url,
            FieldTag::HttpTimeout => self.builtins.http_f_timeout,
            FieldTag::HttpRedirects => self.builtins.http_f_redirects,
            FieldTag::ServerContentType => self.builtins.server_f_content_type,
            FieldTag::CsvHeader => self.builtins.csv_f_header,
            FieldTag::CsvRows => self.builtins.csv_f_rows,
            FieldTag::CacheMaxEntries => self.builtins.cache_f_max_entries,
            FieldTag::CacheTtlMs => self.builtins.cache_f_ttl_ms,
            FieldTag::CacheMaxBytes => self.builtins.cache_f_max_bytes,
            FieldTag::CacheHits => self.builtins.cache_f_hits,
            FieldTag::CacheMisses => self.builtins.cache_f_misses,
            FieldTag::CacheEvictions => self.builtins.cache_f_evictions,
            FieldTag::WsUrl => self.builtins.ws_f_url,
            FieldTag::WsHeaders => self.builtins.ws_f_headers,
            FieldTag::WsTimeout => self.builtins.ws_f_timeout,
            FieldTag::WsPingInterval => self.builtins.ws_f_ping_interval,
            FieldTag::EmailFrom => self.builtins.email_f_from,
            FieldTag::EmailTo => self.builtins.email_f_to,
            FieldTag::EmailCc => self.builtins.email_f_cc,
            FieldTag::EmailBcc => self.builtins.email_f_bcc,
            FieldTag::EmailSubject => self.builtins.email_f_subject,
            FieldTag::EmailTextBody => self.builtins.email_f_text_body,
            FieldTag::EmailHtmlBody => self.builtins.email_f_html_body,
            FieldTag::EmailAttachments => self.builtins.email_f_attachments,
            FieldTag::EmailReplyTo => self.builtins.email_f_reply_to,
            FieldTag::EmailFilename => self.builtins.email_f_filename,
            FieldTag::EmailMimeType => self.builtins.email_f_mime_type,
            FieldTag::EmailContent => self.builtins.email_f_content,
            FieldTag::RetryBaseMs => self.builtins.retry_f_base_ms,
            FieldTag::RetryMaxAttempts => self.builtins.retry_f_max_attempts,
            FieldTag::RetryShouldRetry => self.builtins.retry_f_should_retry,
            FieldTag::RetryStrategy => self.builtins.retry_f_strategy,
            FieldTag::LayoutWrapperAttrs => self.builtins.lw_wrapper_attrs,
            FieldTag::LayoutRootAttrs => self.builtins.lw_root_attrs,
            FieldTag::ButtonOnPress => self.builtins.btn_f_on_press,
            FieldTag::Label => self.builtins.btn_f_label,
            FieldTag::AppInit => self.builtins.live_f_init,
            FieldTag::AppUpdate => self.builtins.live_f_update,
            FieldTag::AppView => self.builtins.live_f_view,
            FieldTag::AppSubscriptions => self.builtins.live_f_subscriptions,
            FieldTag::AppRoutes => self.builtins.live_f_routes,
            FieldTag::AppNotFound => self.builtins.live_f_not_found,
            FieldTag::TerminalOnKey => self.builtins.tui_f_on_key,
            FieldTag::TerminalKeyKind => self.builtins.tui_f_key_kind,
            FieldTag::TerminalKeyValue => self.builtins.tui_f_key_value,
            FieldTag::TerminalOnLine => self.builtins.cli_f_on_line,
            FieldTag::EdgeTop => self.builtins.edge_f_top,
            FieldTag::EdgeRight => self.builtins.edge_f_right,
            FieldTag::EdgeBottom => self.builtins.edge_f_bottom,
            FieldTag::EdgeLeft => self.builtins.edge_f_left,
            FieldTag::InputOnChange => self.builtins.input_f_on_change,
            FieldTag::InputText => self.builtins.input_f_text,
            FieldTag::InputPlaceholder => self.builtins.input_f_placeholder,
            FieldTag::InputIcon => self.builtins.input_f_icon,
            FieldTag::InputChecked => self.builtins.input_f_checked,
            FieldTag::InputSpellcheck => self.builtins.input_f_spellcheck,
            FieldTag::InputValue => self.builtins.input_f_value,
            FieldTag::InputMin => self.builtins.input_f_min,
            FieldTag::InputMax => self.builtins.input_f_max,
            FieldTag::InputStep => self.builtins.input_f_step,
            FieldTag::InputOptions => self.builtins.input_f_options,
            FieldTag::InputSelected => self.builtins.input_f_selected,
            FieldTag::ShadowOffsetX => self.builtins.shadow_f_offset_x,
            FieldTag::ShadowOffsetY => self.builtins.shadow_f_offset_y,
            FieldTag::ShadowBlur => self.builtins.shadow_f_blur,
            FieldTag::ShadowSpread => self.builtins.shadow_f_spread,
            FieldTag::ShadowColor => self.builtins.shadow_f_color,
            FieldTag::ImageSrc => self.builtins.img_f_src,
            FieldTag::ImageDescription => self.builtins.img_f_description,
            FieldTag::ProcessCommand => self.builtins.process_f_command,
            FieldTag::ProcessArgs => self.builtins.process_f_args,
            FieldTag::ProcessCwd => self.builtins.process_f_cwd,
            FieldTag::ProcessEnv => self.builtins.process_f_env,
            FieldTag::ProcessExitCode => self.builtins.process_f_exit_code,
            FieldTag::ProcessStdout => self.builtins.process_f_stdout,
            FieldTag::ProcessStderr => self.builtins.process_f_stderr,
            FieldTag::ProcessCols => self.builtins.process_f_cols,
            FieldTag::ProcessRows => self.builtins.process_f_rows,
            FieldTag::ProcessOutput => self.builtins.process_f_output,
        }
    }

    /// The module path an interpreted [`TyShape::Con`] carries for a given
    /// [`BuiltinTag`], mirroring the exact `Ty::Con { module, .. }` its
    /// `stdlib_scheme` arm builds.
    ///
    /// Every tag is empty-module (unqualified) EXCEPT
    /// [`BuiltinTag::HtmlAttribute`]: it shares the `Attribute` name with
    /// [`BuiltinTag::UiAttribute`] but is module-qualified with the `Html`
    /// constructor symbol, so `ir_type_from_ty`'s disambiguation selects the
    /// `Html` attribute variant that every `Ipe.Html` node kernel takes. Keeping
    /// this the ONE non-empty case preserves byte-identity for the qualified
    /// `Ipe.Html.Attribute` cons while leaving all other interpreted cons
    /// unqualified.
    pub fn builtin_con_module(&self, tag: BuiltinTag) -> Vec<Symbol> {
        match tag {
            BuiltinTag::HtmlAttribute => vec![self.builtins.html_con],
            // The `send` kernel takes `EmailProvider` as its first parameter.
            // Carrying the real `Ipe.Email` home lets a point-free reference to
            // the interpreted scheme lower to the emitted enum; without it the
            // unhomed `Con` misses the lowerer's home-keyed variant lookup and
            // drops into the unknown-builtin internal-compiler-error arm.
            BuiltinTag::EmailProvider => self.builtins.email_home.clone(),
            _ => Vec::new(),
        }
    }

    /// Parse-once type scheme for a stdlib kernel, keyed by the pre-resolved
    /// [`StdlibKernel`] id carried on the `VarKernel` node. `None` = the
    /// kernel has no registry scheme, so the caller
    /// ([`Self::constrain_var_kernel`]) fails closed.
    ///
    /// `Math.min` / `Math.max` are EXCLUDED — they keep their dedicated
    /// `Comparable`-obligation path in `constrain_var_kernel`. The structural
    /// `Ty`-equality against the reference schemes is pinned per-kernel by the
    /// `stdlib_scheme_matches_legacy` parity tripwire, and the covered set is
    /// pinned by `migrated_set_burndown`.
    #[allow(clippy::too_many_lines)] // declarative scheme table — mirrors kernel_ty
    #[allow(clippy::match_same_arms)] // family-grouped declarative type table; merging cross-family arms with coincidentally-equal schemes would obscure the per-family structure
    pub fn stdlib_scheme(&self, k: StdlibKernel) -> Option<Ty> {
        use StdlibKernel as K;
        // Constructors mirror `kernel_ty`'s so the two tables stay byte-faithful
        // (verified structurally by `stdlib_scheme_matches_legacy`).
        let int = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.int,
            args: Vec::new(),
        };
        let float = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.float,
            args: Vec::new(),
        };
        let string = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.string,
            args: Vec::new(),
        };
        let bool_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.bool,
            args: Vec::new(),
        };
        let var = Ty::Var;
        let fun = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b));
        let list = |t: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.list,
            args: vec![t],
        };
        let maybe = |t: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.maybe,
            args: vec![t],
        };
        // `Char` is a zero-argument constructor (runtime rune / `char`).
        let char = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.char,
            args: Vec::new(),
        };
        // ── Scheme-builder closures (produce structurally identical `Ty`
        //    values across the kernel arms; the `stdlib_scheme_matches_legacy`
        //    tripwire proves the equality). ──
        let result = |e: Ty, a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.result,
            args: vec![e, a],
        };
        let dict = |kk: Ty, v: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.dict,
            args: vec![kk, v],
        };
        let set = |a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.set,
            args: vec![a],
        };
        // `Bytes` is a zero-argument constructor.
        let bytes = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.bytes,
            args: Vec::new(),
        };
        // `Decimal` is a zero-argument constructor (Ipe.Decimal).
        let decimal = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.decimal,
            args: Vec::new(),
        };
        let error_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.error,
            args: Vec::new(),
        };
        // `ErrorKind` is a zero-argument constructor (the 11-variant kind union).
        let errorkind_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.errorkind,
            args: Vec::new(),
        };
        // `ErrorDetails` is a zero-argument constructor.
        let errordetails_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.errordetails,
            args: Vec::new(),
        };
        let tuple2 = |a: Ty, b: Ty| Ty::Tuple(vec![a, b]);
        // `task(a)` — `Task a` (the error channel is the implicit `IpeError`).
        let task = |a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.task,
            args: vec![a],
        };
        let task_unit = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.task,
            args: vec![Ty::Unit],
        };
        // Opaque app-leaf constructors — nullary, no type arguments.
        let web_app_leaf = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.web_app,
            args: Vec::new(),
        };
        let tui_app_leaf = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.tui_app,
            args: Vec::new(),
        };
        let cli_app_leaf = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.cli_app,
            args: Vec::new(),
        };
        let cmd = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.cmd,
            args: vec![m],
        };
        let sub = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.sub,
            args: vec![m],
        };
        // `topic(a)` — `Topic a` — the phantom topic-handle type.
        // Erases to `String` at runtime; used only in kernel type schemes so
        // that publisher and subscriber share the same payload type variable.
        let topic = |a: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.topic_con,
            args: vec![a],
        };
        // `dec(inner)` — `Decoder inner` — the opaque row-decoder type shared by
        // JSON decode and Db.Decode.
        let dec = |inner: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.decoder,
            args: vec![inner],
        };
        // `Cond row` — the typed `WHERE`-predicate ADT (`Ipe.Db.Store`), the
        // result of the accessor-typed equality leaf. The `row` argument ties the
        // predicate to the store's row type: the getter arrow `(row -> value)`
        // shares `row` with this result, so `where`/`updateWhere`/`deleteWhere`
        // (typed `Cond a -> … Store a`) pin the accessor's record to the store's
        // row, making a cross-row column or a value-type mismatch a type error.
        let cond = |row: Ty| Ty::Con {
            module: self.builtins.db_store_home.clone(),
            name: self.builtins.cond_con,
            args: vec![row],
        };
        // `Store row` — the `Ipe.Db.Store.Store a` ADT. The `row` type variable
        // ties the store's row type to the accessor's record type in the
        // column-spec builder kernel schemes, so a cross-row accessor is a
        // type mismatch rather than a silent wrong column.
        let store = |row: Ty| Ty::Con {
            module: self.builtins.db_store_home.clone(),
            name: self.builtins.store_con,
            args: vec![row],
        };
        // `Draft row` — the `Ipe.Db.Store.Draft a` unclassified-table ADT. The
        // schema-shaping column-spec builders (`primaryKey` / `serial` / … /
        // `defaultInt`) take and return a `Draft row`, so they refine the table
        // before classification; no read/write kernel accepts a `Draft`, so an
        // unclassified table is unqueryable by construction (deny-by-default).
        let draft = |row: Ty| Ty::Con {
            module: self.builtins.db_store_home.clone(),
            name: self.builtins.draft_con,
            args: vec![row],
        };
        // `Joined a b` — the two-store inner-join ADT (`Ipe.Db.Store`), the
        // result of `Store.join`. It carries both sides' row types so `toList`
        // returns `(a, b)` pairs decoded through each store's own codec.
        let joined = |a: Ty, b: Ty| Ty::Con {
            module: self.builtins.db_store_home.clone(),
            name: self.builtins.joined_con,
            args: vec![a, b],
        };
        // `Select row` — the column-projection ADT (`Ipe.Db.Store`), the result
        // of `Store.select`. `row` is the projected shape the lambda returns; it
        // is phantom over the runtime value (which carries only query data) but
        // records the shape so `selectToList` / `selectToMaybe` return the typed
        // `row` — the concrete decode is emitted at the call site from it.
        let select = |row: Ty| Ty::Con {
            module: self.builtins.db_store_home.clone(),
            name: self.builtins.select_con,
            args: vec![row],
        };
        // `Policy row` — the row-security policy algebra ADT (`Ipe.Db.Store`),
        // the result of the accessor-typed policy builders. The `row` argument
        // is phantom over the rule data but shares the accessor's record type,
        // so `secured` (`Policy row -> Store row -> …`) pins the policy's
        // columns to the store's row, making a cross-row policy column a type
        // error rather than a silent wrong column.
        let policy = |row: Ty| Ty::Con {
            module: self.builtins.db_store_home.clone(),
            name: self.builtins.policy_con,
            args: vec![row],
        };
        // `Order` — the `Ipe.Db.Store.Order` nullary ADT (`Asc | Desc`). The
        // `orderByLeft` / `orderByRight` scheme names it directly so the
        // type-checker requires the caller to pass an `Order`, not an
        // arbitrary type. It has no type parameters.
        let order = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.order_con,
            args: Vec::new(),
        };
        // `Codec inner` — the `Ipe.Codec` codec ADT, the first parameter of
        // `Store.eqBy`. Its real home unifies with the user's `Ipe.Codec.Codec`
        // and lets a point-free reference to the scheme lower to the emitted enum.
        let codec = |inner: Ty| Ty::Con {
            module: self.builtins.codec_home.clone(),
            name: self.builtins.codec_con,
            args: vec![inner],
        };
        // Opaque nullary type constructors (mirror `kernel_ty`'s inline `Ty::Con`s).
        let db = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.db,
            args: Vec::new(),
        };
        // `Ipe.Db.Migration` is a record alias `{ name : String, sql : String }`.
        // `Db.migrate` schemes over `List Migration`, and `Db.defaultMigration`
        // returns one — so a program can build migrations as record literals. The
        // record folds to a synthesised `Rec…` struct; the `DbMigrate` emit
        // converts each to a `(name, sql)` tuple for the `db_migrate_apply`
        // runtime kernel.
        let migration = || {
            let string = || Ty::Con {
                module: Vec::new(),
                name: self.builtins.string,
                args: Vec::new(),
            };
            let mut m_fields = BTreeMap::new();
            m_fields.insert(self.builtins.migration_f_name, string());
            m_fields.insert(self.builtins.migration_f_sql, string());
            Ty::Record(m_fields, RowTail::Closed)
        };
        let sqlvalue = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.sqlvalue,
            args: Vec::new(),
        };
        let sqlfield = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.sqlfield,
            args: Vec::new(),
        };
        // `SqlFragment` — `Ipe.Db.Sql`'s opaque WHERE-fragment type.
        let sqlfragment = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.sqlfragment,
            args: Vec::new(),
        };
        // `ProjectionTerm` — the typed column-projection ADT for
        // `Ipe.Db.Store.selectNamed`.
        let projection_term = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.projection_term,
            args: Vec::new(),
        };
        // `Secret` — `Ipe.Secret`'s opaque sealed secret-string type.
        let secret = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.secret,
            args: Vec::new(),
        };
        // `Path` — `Ipe.Path`'s opaque validated filesystem-path type.
        let path = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.path,
            args: Vec::new(),
        };
        // `Regex` — `Ipe.Regex`'s opaque compiled-pattern handle.
        let regex = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.regex,
            args: Vec::new(),
        };
        let req = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.server_request,
            args: Vec::new(),
        };
        // `Ipe.Http.Server.Response` is a record alias `{ status : Int, body :
        // String, headers : Dict String String, contentType : String }`
        // (reference `Ipê/Http/Server.ipe:66`), NOT an opaque nominal. Every
        // server kernel that produces/consumes a `Response` schemes over this
        // record so a handler-built record literal — and a field read off a
        // `Response` — unify with the kernel signatures. The record folds to the
        // runtime `IrType::ServerResponse` struct at lowering (see
        // `ipe_lower::is_server_response_shape`).
        let resp = || {
            let string = || Ty::Con {
                module: Vec::new(),
                name: self.builtins.string,
                args: Vec::new(),
            };
            let mut resp_fields = BTreeMap::new();
            resp_fields.insert(self.builtins.http_f_body, string());
            resp_fields.insert(self.builtins.server_f_content_type, string());
            resp_fields.insert(
                self.builtins.http_f_headers,
                Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.dict,
                    args: vec![string(), string()],
                },
            );
            resp_fields.insert(
                self.builtins.http_f_status,
                Ty::Con {
                    module: Vec::new(),
                    name: self.builtins.int,
                    args: Vec::new(),
                },
            );
            Ty::Record(resp_fields, RowTail::Closed)
        };
        let route = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.server_route,
            args: Vec::new(),
        };
        let cookie = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.server_cookie,
            args: Vec::new(),
        };
        // `auth_config()` / `token_source()` — the opaque authed-route
        // descriptors. Built only through the `Server` auth kernels (maps to
        // `IrType::AuthConfig` / `IrType::TokenSource`).
        let auth_config = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.auth_config,
            args: Vec::new(),
        };
        let token_source = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.token_source,
            args: Vec::new(),
        };
        // `sw()` — the opaque `StreamWriter` handle. Used by
        // `Stream.stream` callback arg and `Stream.emit`/`finish`/`withContentType`.
        let sw = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.stream_writer,
            args: Vec::new(),
        };
        // `stream_id()` — the opaque `StreamId` handle from
        // `Ipe.Http.Stream`. Backed at runtime by
        // `ipe_runtime::http_stream::IpeStreamId` (a newtype over `i64`).
        // Used as the return type of `HttpStream.open` and the first argument
        // of `forEachChunk`, `close`, and `chunks`.
        let stream_id = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.stream_id,
            args: Vec::new(),
        };
        // `wsh()` — the opaque `WsHandle` per-peer handle.
        // Used as the first arg of every WsServerCfg callback and as the
        // target of `sendToClient` / `sendBinaryToClient` / `broadcast` / `closeClient`.
        let wsh = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.ws_server,
            args: Vec::new(),
        };
        // `wscfg()` — the opaque `WsServerCfg<IpeError>` configuration type.
        // Built by `Ws.defaultCfg` and threaded through the builder chain.
        let wscfg = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.ws_server_cfg,
            args: Vec::new(),
        };
        // ── stdlib record / opaque-Con helpers ─────────────────────────
        // `Csv` — closed record `{ header : List String, rows : List (List
        // String) }` (runtime `ipe_runtime::csv::CsvDoc`).
        let csv_rec = || {
            let mut m = BTreeMap::new();
            m.insert(self.builtins.csv_f_header, list(string()));
            m.insert(self.builtins.csv_f_rows, list(list(string())));
            Ty::Record(m, RowTail::Closed)
        };
        // `CacheCfg` — closed record `{ maxEntries : Int, ttlMs : Int,
        // maxBytes : Int }`. The lowerer folds a value of this exact shape to the
        // nominal `IrType::CacheCfg` (`ipe_runtime::cache::CacheCfg`) so a
        // `Cache.defaultCfg`-built record literal constructs the runtime struct
        // the `cache_new_raw` kernel takes (mirrors the `HttpRequest` fold).
        let cachecfg_rec = || {
            let mut m = BTreeMap::new();
            m.insert(self.builtins.cache_f_max_entries, int());
            m.insert(self.builtins.cache_f_ttl_ms, int());
            m.insert(self.builtins.cache_f_max_bytes, int());
            Ty::Record(m, RowTail::Closed)
        };
        // `Cache.stats` return — closed record `{ hits : Int,
        // misses : Int, evictions : Int }` (runtime `ipe_runtime::cache::
        // CacheStats`). Consumed by field access on the kernel result, exactly
        // like `Csv`'s `CsvDoc` return, so no lowerer fold is needed on this side.
        let cache_stats_rec = || {
            let mut m = BTreeMap::new();
            m.insert(self.builtins.cache_f_hits, int());
            m.insert(self.builtins.cache_f_misses, int());
            m.insert(self.builtins.cache_f_evictions, int());
            Ty::Record(m, RowTail::Closed)
        };
        // `WebSocketCfg` — closed record `{ url : String, headers :
        // List (String, String), timeout : Int, pingInterval : Int }`. The
        // lowerer folds a value of this exact shape to the nominal
        // `IrType::WebSocketClientCfg` (`ipe_runtime::ws_client::WsClientCfg`) so
        // a `WebSocket.defaultCfg`-built record literal constructs the runtime
        // struct the `web_socket_connect_with` kernel takes (mirrors the
        // `HttpRequest` / `CacheCfg` folds).
        let wsclientcfg = || {
            let mut m = BTreeMap::new();
            m.insert(self.builtins.ws_f_url, string());
            m.insert(self.builtins.ws_f_headers, list(tuple2(string(), string())));
            m.insert(self.builtins.ws_f_timeout, int());
            m.insert(self.builtins.ws_f_ping_interval, int());
            Ty::Record(m, RowTail::Closed)
        };
        // Ipe.Email: `EmailProvider` closed ADT (runtime
        // `ipe_runtime::email::EmailProvider`). The `send` kernel takes this as
        // its first parameter; carrying the real `Ipe.Email` home lets a
        // point-free reference to the scheme lower to the emitted enum instead
        // of dropping into the lowerer's unknown-builtin internal-compiler-error
        // arm. Empty-home unifies with the user's declared type, but only the
        // real home keys the home-sensitive variant lookup.
        let email_provider = || Ty::Con {
            module: self.builtins.email_home.clone(),
            name: self.builtins.email_provider,
            args: Vec::new(),
        };
        // `Key` — opaque role-typed crypto key (`ipe_runtime::crypto::Key`).
        // The ONLY constructor is `Key.fromString`/`Key.fromBytes`; no implicit
        // `String` coercion.  Lowered to `IrType::CryptoKey`.
        let crypto_key = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.crypto_key,
            args: Vec::new(),
        };
        // `Mac` — opaque role-typed MAC output (`ipe_runtime::crypto::Mac`).
        // Produced exclusively by the `*WithKey` kernels; extracted via `Mac.toHex`.
        // Lowered to `IrType::CryptoMac`.
        let crypto_mac = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.crypto_mac,
            args: Vec::new(),
        };
        // `EmailAddress` — opaque validated email address
        // (`ipe_runtime::email::EmailAddress`).  The ONLY constructor is
        // `EmailAddress.parse`; extracted via `EmailAddress.toString`.
        // Lowered to `IrType::EmailAddress`.
        let email_address = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.email_address,
            args: Vec::new(),
        };
        // `Url` — `Ipe.Url`'s opaque validated URL type
        // (`ipe_runtime::url::Url`). The ONLY constructor is `Url.fromString`;
        // extracted via `Url.toString`. Lowered to `IrType::Url`.
        let url = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.url,
            args: Vec::new(),
        };
        // `Dsn` — opaque validated connection descriptor
        // (`ipe_runtime::dsn::Dsn`). Constructed only by `Db.Dsn.parse` /
        // `Db.Dsn.build`; lowered to `IrType::Dsn`.
        let dsn = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.dsn,
            args: Vec::new(),
        };
        // External `Connection mode` — the read-only-by-type foreign-DB handle
        // (`ipe_runtime::external_conn::ExternalConnection`). The phantom `mode`
        // (`ReadOnly` / `ReadWrite`) is a real type at inference so a read-only
        // value cannot unify into a write kernel; erased at emit.
        let conn_read_only = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.conn_read_only,
            args: Vec::new(),
        };
        let conn_read_write = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.conn_read_write,
            args: Vec::new(),
        };
        let connection = |mode: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.connection,
            args: vec![mode],
        };
        // Runtime-config `Setting shape` carrier and its phantom shape markers.
        // The marker (`Web` / `WebView` / `Terminal`, or a free var for a
        // cross-cutting setting) is a real type at inference so a `Web`-only
        // setting cannot unify into a `Terminal` app's settings list; erased at
        // emit (one concrete `ipe_runtime::app_config::Setting` per position).
        let shape_web = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.shape_web,
            args: Vec::new(),
        };
        let setting = |shape: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.setting,
            args: vec![shape],
        };
        // The closed config-tag ADTs — the argument types of `Host.bind` /
        // `Log.level` / `Web.csrf`. Each is nullary; a value comes only from its
        // constructor kernels, which project to the raw `Int` tag at emit. A bare
        // `Int` no longer unifies here, so an out-of-range tag is a type error.
        let host_mode = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.host_mode,
            args: Vec::new(),
        };
        let log_level = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.log_level,
            args: Vec::new(),
        };
        let csrf_mode = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.csrf_mode,
            args: Vec::new(),
        };
        let revocation_mode = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.revocation_mode,
            args: Vec::new(),
        };
        // `Locale` — opaque BCP-47 locale handle
        // (`ipe_runtime::locale::Locale`).  The ONLY constructor is
        // `Locale.fromTag`; extracted via `Locale.toTag`.
        // Lowered to `IrType::Locale`.
        let locale = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.locale,
            args: Vec::new(),
        };
        // `EmailMessage` — closed 9-field record (runtime
        // `ipe_runtime::email::EmailMessage`). The lowerer folds a value of this
        // exact shape to the nominal `IrType::EmailMessage` so a
        // `defaultMessage`-built record literal constructs the runtime struct the
        // `email_send` kernel takes (mirrors the `CsvDoc` / `CacheCfg` folds).
        let email_message_rec = || {
            let mut m = BTreeMap::new();
            m.insert(self.builtins.email_f_from, email_address());
            m.insert(self.builtins.email_f_to, list(email_address()));
            m.insert(self.builtins.email_f_cc, list(email_address()));
            m.insert(self.builtins.email_f_bcc, list(email_address()));
            m.insert(self.builtins.email_f_subject, string());
            m.insert(self.builtins.email_f_text_body, string());
            m.insert(self.builtins.email_f_html_body, string());
            // `attachments : List Attachment` — the element is the runtime
            // `EmailAttachment` record shape `{ filename, mimeType, content }`.
            let mut att = BTreeMap::new();
            att.insert(self.builtins.email_f_filename, string());
            att.insert(self.builtins.email_f_mime_type, string());
            att.insert(self.builtins.email_f_content, bytes());
            m.insert(
                self.builtins.email_f_attachments,
                list(Ty::Record(att, RowTail::Closed)),
            );
            m.insert(self.builtins.email_f_reply_to, email_address());
            Ty::Record(m, RowTail::Closed)
        };
        let attr = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.attribute,
            args: vec![m],
        };
        // `Ipe.Html.Attribute` — SAME name as `Ipe.Ui.Attribute` (`attr` above)
        // but module-qualified with `html_con`, so `ir_type_from_ty`'s T2
        // disambiguation selects `HtmlAttribute`, matching the runtime
        // `Vec<html::Attribute<M>>` that every Ipe.Html node kernel takes
        // (div/span/a/button/p/input/img/node/styleNode/attrToString). Using the
        // bare Ui `attr` for these would mis-select the Ui attribute variant.
        let html_attr = |m: Ty| Ty::Con {
            module: vec![self.builtins.html_con],
            name: self.builtins.attribute,
            args: vec![m],
        };
        let elem_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.element,
            args: vec![m],
        };
        let cells_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.cells,
            args: vec![m],
        };
        // `tui_attr(msg)` — the cell-native attribute type `Ipe.Tea.Tui.Ui.Attribute msg`.
        // Distinct from the DOM `attr` above, so `Screen`-view builders never
        // admit a DOM attribute.
        let tui_attr = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.tui_attr,
            args: vec![m],
        };
        // `lines_t(msg)` — the Cli line-oriented view type `Lines msg`. Distinct
        // from both `Element msg` and `Screen msg`.
        let lines_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.cli_lines,
            args: vec![m],
        };
        // `cli_attr(msg)` — the line-native attribute type
        // `Ipe.Tea.Cli.Ui.Attribute msg`. Distinct from the DOM `attr` and the
        // cell-native `tui_attr`, so a `Lines`-view builder admits neither.
        let cli_attr = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.cli_attr,
            args: vec![m],
        };
        // `term_color()` — the first-class terminal palette `Terminal.Color`.
        let term_color = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.term_color,
            args: Vec::new(),
        };
        // `custom_element(down, up)` — the empty-home JS-widget boundary handle
        // `CustomElement down up`, the argument type of `Ui.widget`.
        let custom_element = |down: Ty, up: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.custom_element,
            args: vec![down, up],
        };
        let html_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.html_con,
            args: vec![m],
        };
        // `label_t(msg)` — `Label msg` from `Ipe.Ui.Input`.
        // Lowered to `IrType::Ui { ctor: UiCtor::Label, msg }` via the `"Label"`
        // arm in `ipe_lower::ir_type_from_ty`. The type carries the module path
        // `[input_con]` so it doesn't collide with any user `type Label`.
        // (We use an empty module here because `ir_type_from_ty` routes all
        // unqualified `"Label"` cons to `UiCtor::Label` regardless — the name is
        // reserved in the kernel namespace and never appears as a user type.)
        let label_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.input_label_con,
            args: vec![m],
        };
        // `placeholder_t(msg)` — `Placeholder msg` from `Ipe.Ui.Input`.
        // Lowered to `IrType::Ui { ctor: UiCtor::Placeholder, msg }`.
        let placeholder_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.input_placeholder_con,
            args: vec![m],
        };
        // `radio_option_t(msg)` — `RadioOption msg` from `Ipe.Ui.Input`.
        // Lowered to `IrType::Ui { ctor: UiCtor::RadioOption, msg }`.
        let radio_option_t = |m: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.input_radio_option_con,
            args: vec![m],
        };
        // Nullary Ipe.Ui plain types (`Length` / `Color`) — lowered to
        // `IrType::UiPlain(UiPlain::Length | UiPlain::Color)`.
        let length = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.length,
            args: Vec::new(),
        };
        let color = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.color,
            args: Vec::new(),
        };
        // `description()` — the opaque `Description` semantic-description type
        // produced by `Ui.descMain` / `Ui.descHeading` / …. Lowered to
        // `IrType::UiPlain(UiPlain::Description)`.
        let description = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.description,
            args: Vec::new(),
        };
        // `pseudo_class()` — the opaque `PseudoClass` selector-tag type produced
        // by `Ui.hover` / `Ui.focus` / `Ui.focusVisible` / `Ui.active` /
        // `Ui.disabled` and consumed by `Ui.onPseudo`. Lowered to
        // `IrType::UiPlain(UiPlain::PseudoClass)`.
        let pseudo_class = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.pseudo_class,
            args: Vec::new(),
        };
        // `value()` — the opaque `Value = any` JSON node produced/consumed by the
        // `JsonEnc.*` encoders. Lowered to `IrType::Json` (`JsonVal`).
        let value = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.json_value,
            args: Vec::new(),
        };
        // ── JWT builder opaque types (D-00) ─────────────────────────────────
        // `claims_ty()` — opaque JWT claims accumulator.  Backed at runtime by
        // `serde_json::Value` (maps to `IrType::Json` in the lowerer).
        let claims_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.jwt_claims,
            args: Vec::new(),
        };
        // `algorithm_ty()` — JWT signing algorithm descriptor.  Backed at
        // runtime by a sealed `Ipe.Secret` wrapping the string
        // `"HS256:<secret>"` or `"RS256:<pem>"` (maps to `IrType::Secret` in
        // the lowerer) — the key material never gets a `Debug`/`Display`/
        // stringify surface, mirroring `Ipe.Secret` itself.
        let algorithm_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.jwt_algorithm,
            args: Vec::new(),
        };
        // `principal_ty()` — the opaque authenticated subject. Backed at runtime
        // by `ipe_runtime::principal::Principal` (maps to `IrType::Principal`).
        // No Ipê constructor: a value only ever comes from the auth middleware
        // mint, so no term of this type can be built in Ipê.
        let principal_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.principal,
            args: Vec::new(),
        };
        let web_req = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.web_req,
            args: Vec::new(),
        };
        // `session_handle()` — the opaque `SessionHandle` from `Ipe.Ffi.Js`, a
        // nullary con obtained only from `Js.openSession`. Backed by the runtime
        // session id (`IrType::SessionHandle`, renders to `i64`).
        let session_handle = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.session_handle,
            args: Vec::new(),
        };
        // `live_route(page)` — `WebRoute page` is parametric on the page type.
        // Its purpose is to carry the page type through HM unification so that
        //   routes : List (WebRoute var(2))            [K::WebApp]
        //   Web.route : String -> builder -> WebRoute page  [K::WebRoute]
        //   notFound : var(2)
        // all share ONE page type variable.  A `notFound = 5` in a routed app
        // that also uses `Web.route "/" CounterPage` sets `var(2) = Page`
        // (through the per-route witness — see [`RouteWitnessCheck`]) and then
        // forces `5 : Page` → IPE-T0001.  Seal fix — the
        // "exit-0-then-cargo-fail E0308" class.  Since round 4 the arg is no
        // longer phantom at the IR level: the lowerer threads it into
        // `IrType::WebRoute(page)` so the backend renders `Route<Page>`.
        let live_route = |page: Ty| Ty::Con {
            module: Vec::new(),
            name: self.builtins.live_route_con,
            args: vec![page],
        };
        // `HttpResponse = { body : String, headers : Dict String String, status : Int }`
        let http_response = || {
            let mut resp_fields = BTreeMap::new();
            resp_fields.insert(self.builtins.http_f_body, string());
            resp_fields.insert(self.builtins.http_f_headers, dict(string(), string()));
            resp_fields.insert(self.builtins.http_f_status, int());
            Ty::Record(resp_fields, RowTail::Closed)
        };
        // `HttpMethod` — the closed ADT (`Get | Post | Put | Delete | Patch |
        // Head | Options`).  Like `Order` and `Decimal`, it is known to the
        // type system as a zero-argument constructor with an empty module path
        // (builtins-like treatment; the Ipê source defines it in `Ipe.Http`
        // but the compiler folds it as a pre-interned nominal, analogous to
        // how `Order` is defined in `Ipe.Basics` but treated as a builtin here).
        let http_method_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.http_method,
            args: Vec::new(),
        };
        // `HttpRequest = { body, headers, method, redirects, timeout, url }`
        let redirect_policy_ty = || Ty::Con {
            module: Vec::new(),
            name: self.builtins.redirect_policy,
            args: Vec::new(),
        };
        let http_request = || {
            let mut req_fields = BTreeMap::new();
            req_fields.insert(self.builtins.http_f_body, string());
            req_fields.insert(
                self.builtins.http_f_headers,
                list(tuple2(string(), string())),
            );
            req_fields.insert(self.builtins.http_f_method, http_method_ty());
            req_fields.insert(self.builtins.http_f_redirects, redirect_policy_ty());
            req_fields.insert(self.builtins.http_f_timeout, int());
            req_fields.insert(self.builtins.http_f_url, string());
            Ty::Record(req_fields, RowTail::Closed)
        };
        // `BackoffStrategy` — the four-constructor retry-strategy ADT.
        let backoff_strategy = || Ty::Con {
            module: vec![],
            name: self.builtins.backoffstrategy,
            args: vec![],
        };
        // `RetryPolicy e = { baseMs : Int, maxAttempts : Int,
        //                    shouldRetry : e -> Bool, strategy : BackoffStrategy }`
        // Fields sorted alphabetically (BTreeMap order) — matches the emitted Rust struct.
        let retry_policy = |e: Ty| {
            let mut rp_fields = BTreeMap::new();
            rp_fields.insert(self.builtins.retry_f_base_ms, int());
            rp_fields.insert(self.builtins.retry_f_max_attempts, int());
            rp_fields.insert(self.builtins.retry_f_should_retry, fun(e, bool_ty()));
            rp_fields.insert(self.builtins.retry_f_strategy, backoff_strategy());
            Ty::Record(rp_fields, RowTail::Closed)
        };
        Some(match k {
            // ── List (kernel-anchored combinators) ──
            // map : (a -> b) -> List a -> List b
            K::ListMap => fun(fun(var(0), var(1)), fun(list(var(0)), list(var(1)))),
            // filter : (a -> Bool) -> List a -> List a
            K::ListFilter => fun(fun(var(0), bool_ty()), fun(list(var(0)), list(var(0)))),
            // foldl / foldr : (a -> b -> b) -> b -> List a -> b
            K::ListFoldl | K::ListFoldr => fun(
                fun(var(0), fun(var(1), var(1))),
                fun(var(1), fun(list(var(0)), var(1))),
            ),
            // length : List a -> Int
            K::ListLength => fun(list(var(0)), int()),
            // head : List a -> Maybe a
            K::ListHead => fun(list(var(0)), maybe(var(0))),
            // tail : List a -> Maybe (List a)
            K::ListTail => fun(list(var(0)), maybe(list(var(0)))),
            // member : a -> List a -> Bool
            K::ListMember => fun(var(0), fun(list(var(0)), bool_ty())),
            // range : Int -> Int -> List Int
            K::ListRange => fun(int(), fun(int(), list(int()))),
            // reverse : List a -> List a
            K::ListReverse => fun(list(var(0)), list(var(0))),
            // append : List a -> List a -> List a
            K::ListAppend => fun(list(var(0)), fun(list(var(0)), list(var(0)))),
            // concat : List (List a) -> List a
            K::ListConcat => fun(list(list(var(0))), list(var(0))),
            // take : Int -> List a -> List a
            K::ListTake => fun(int(), fun(list(var(0)), list(var(0)))),
            // drop : Int -> List a -> List a
            K::ListDrop => fun(int(), fun(list(var(0)), list(var(0)))),
            // zip : List a -> List b -> List (a, b)
            K::ListZip => fun(
                list(var(0)),
                fun(list(var(1)), list(tuple2(var(0), var(1)))),
            ),
            // cons : a -> List a -> List a
            K::ListCons => fun(var(0), fun(list(var(0)), list(var(0)))),
            // isEmpty : List a -> Bool
            K::ListIsEmpty => fun(list(var(0)), bool_ty()),
            // concatMap : (a -> List b) -> List a -> List b
            K::ListConcatMap => fun(fun(var(0), list(var(1))), fun(list(var(0)), list(var(1)))),
            // indexedMap : (Int -> a -> b) -> List a -> List b
            K::ListIndexedMap => fun(
                fun(int(), fun(var(0), var(1))),
                fun(list(var(0)), list(var(1))),
            ),
            // any / all : (a -> Bool) -> List a -> Bool
            K::ListAny | K::ListAll => fun(fun(var(0), bool_ty()), fun(list(var(0)), bool_ty())),
            // find : (a -> Bool) -> List a -> Maybe a
            K::ListFind => fun(fun(var(0), bool_ty()), fun(list(var(0)), maybe(var(0)))),
            // ── List batch ────────────────────────────────────────────
            // filterMap : (a -> Maybe b) -> List a -> List b
            K::ListFilterMap => fun(fun(var(0), maybe(var(1))), fun(list(var(0)), list(var(1)))),
            // sortBy : (a -> comparable) -> List a -> List a — BASE scheme only.
            // var(0)=a (element), var(1)=key type (Comparable obligation layered in
            // constrain_var_kernel, keyed off id, same pattern as MathMin/MathMax).
            // Production never reaches this arm (obligation pre-check early-returns
            // the bounded scheme); it exists so `stdlib_scheme` is total.
            K::ListSortBy => fun(fun(var(0), var(1)), fun(list(var(0)), list(var(0)))),
            // sort : comparable a => List a -> List a — BASE scheme only (Ord
            // obligation layered in `constrain_var_kernel`, keyed off id).
            K::ListSort => fun(list(var(0)), list(var(0))),
            // sortWith : (a -> a -> Order) -> List a -> List a — fully generic
            // (the comparator supplies the ordering), so no obligation is needed.
            K::ListSortWith => fun(
                fun(var(0), fun(var(0), order())),
                fun(list(var(0)), list(var(0))),
            ),
            // singleton : a -> List a
            K::ListSingleton => fun(var(0), list(var(0))),
            // repeat : Int -> a -> List a
            K::ListRepeat => fun(int(), fun(var(0), list(var(0)))),
            // sum / product : number a => List a -> a — BASE scheme only
            // (number obligation layered in `constrain_var_kernel`).
            K::ListSum | K::ListProduct => fun(list(var(0)), var(0)),
            // maximum / minimum : comparable a => List a -> Maybe a — BASE
            // scheme only (Ord obligation layered in `constrain_var_kernel`).
            K::ListMaximum | K::ListMinimum => fun(list(var(0)), maybe(var(0))),
            // unique : List a -> List a — fully generic (equality-only, tested
            // with `==` by the runtime; no Ord/Hash obligation, exactly like
            // `List.member`), so the scheme needs no bounded var.
            K::ListUnique => fun(list(var(0)), list(var(0))),
            // intersperse : a -> List a -> List a
            K::ListIntersperse => fun(var(0), fun(list(var(0)), list(var(0)))),
            // partition : (a -> Bool) -> List a -> (List a, List a)
            K::ListPartition => fun(
                fun(var(0), bool_ty()),
                fun(list(var(0)), tuple2(list(var(0)), list(var(0)))),
            ),
            // unzip : List (a, b) -> (List a, List b)
            K::ListUnzip => fun(
                list(tuple2(var(0), var(1))),
                tuple2(list(var(0)), list(var(1))),
            ),
            // map2 : (a -> b -> r) -> List a -> List b -> List r.
            // vars: 0=a, 1=b, 2=r.
            K::ListMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(list(var(0)), fun(list(var(1)), list(var(2)))),
            ),
            // map3 : (a -> b -> c -> r) -> List a -> List b -> List c -> List r.
            // vars: 0=a, 1=b, 2=c, 3=r.
            K::ListMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(
                    list(var(0)),
                    fun(list(var(1)), fun(list(var(2)), list(var(3)))),
                ),
            ),
            // map4 : (a -> b -> c -> d -> r) -> List a..d -> List r. vars 0..4.
            K::ListMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    list(var(0)),
                    fun(
                        list(var(1)),
                        fun(list(var(2)), fun(list(var(3)), list(var(4)))),
                    ),
                ),
            ),
            // map5 : (a -> b -> c -> d -> e -> r) -> List a..e -> List r. vars 0..5.
            K::ListMap5 => fun(
                fun(
                    var(0),
                    fun(var(1), fun(var(2), fun(var(3), fun(var(4), var(5))))),
                ),
                fun(
                    list(var(0)),
                    fun(
                        list(var(1)),
                        fun(
                            list(var(2)),
                            fun(list(var(3)), fun(list(var(4)), list(var(5)))),
                        ),
                    ),
                ),
            ),

            // ── Basics core Prelude (6 — slice) ──
            K::BasicsIdentity => fun(var(0), var(0)),
            K::BasicsAlways => fun(var(0), fun(var(1), var(0))),
            K::BasicsFst => fun(tuple2(var(0), var(1)), var(0)),
            K::BasicsSnd => fun(tuple2(var(0), var(1)), var(1)),
            K::BasicsModBy => fun(int(), fun(int(), int())),
            // `clamp : comparable -> comparable -> comparable -> comparable`.
            // BASE scheme only — three independent `var(0)`s; the shared
            // `Comparable a` (Ord) obligation is layered on in
            // `constrain_var_kernel` (keyed off id), exactly as `Math.min` /
            // `Math.max`. Production never reaches this arm (the obligation
            // pre-check early-returns the bounded scheme); it exists so
            // `stdlib_scheme` is total and the burndown tripwire holds.
            K::BasicsClamp => fun(var(0), fun(var(0), fun(var(0), var(0)))),
            // toString : a -> String — base scheme for the totality gate; the
            // real STRINGIFY-bounded typing is direct-built in constrain_var_kernel,
            // same pattern as clamp/min/max.
            K::BasicsToString => fun(var(0), string()),
            // ── Basics numerics ────────────────────────────────────────
            // negate / abs: `number a => a -> a`. BASE scheme only (bounded scheme
            // is direct-built in constrain_var_kernel). Production never reaches
            // this arm (obligation pre-check early-returns); exists for the totality
            // gate (`stdlib_scheme_total_over_reachable`).
            K::BasicsNegate | K::BasicsAbs => fun(var(0), var(0)),
            // sqrt : Float -> Float — monomorphic, no obligation pre-check needed.
            // min / max: `comparable a => a -> a -> a`. BASE scheme only (bounded
            // scheme is direct-built in constrain_var_kernel, same as MathMin/MathMax).
            K::BasicsMin | K::BasicsMax => fun(var(0), fun(var(0), var(0))),
            // `compare`: base scheme (production hits the direct-build in
            // constrain_var_kernel; this arm exists for the totality gate).
            K::BasicsCompare => fun(var(0), fun(var(0), order())),
            // ── end Basics numerics ────────────────────────────────────

            // ── Math (min / max stay on the obligation path — NOT migrated) ──
            // Constants — bare Float values (arity 0).
            // isNaN : Float -> Bool.
            // abs : Int -> Int.
            // Arity-1 Float -> Float.
            // Arity-1 Float -> Int (rounding functions).
            // Arity-2 Float -> Float -> Float.
            // Math.min / max — BASE scheme only (the `Comparable a` obligation is
            // layered on top in `constrain_var_kernel`, keyed off the id). The
            // parity tripwire checks this base against `kernel_ty("Math","min")`;
            // production never reaches this arm for min/max (the obligation
            // pre-check early-returns the bounded scheme).
            K::MathMin | K::MathMax => fun(var(0), fun(var(0), var(0))),

            // ── Random seeded (Generator primitives) — pure, reproducible ──
            // seededIntRaw : Int -> Int -> Int -> (Int, Int)   (seed, lo, hi) → (value, nextSeed)
            K::RandomSeededInt => fun(int(), fun(int(), fun(int(), tuple2(int(), int())))),
            // seededFloatRaw : Int -> (Float, Int)             seed → (value, nextSeed)
            K::RandomSeededFloat => fun(int(), tuple2(float(), int())),
            // seededChoiceRaw : Int -> List a -> (Maybe a, Int)  (seed, list) → (choice, nextSeed)
            K::RandomSeededChoice => {
                fun(int(), fun(list(var(0)), tuple2(maybe(var(0)), int())))
            }

            // ── Log ──
            // info/debug/warn/error : String -> Task Error (). The
            // *With variants (List (String, a) attrs) are Stringify-bounded and
            // stay fail-closed until a Stringify obligation is added.
            K::LogInfo | K::LogDebug | K::LogWarn | K::LogError => fun(string(), task_unit()),
            // *With : String -> List a -> Task Error () where `a` is Stringify.
            // Base scheme for the totality gate; the Stringify obligation on the
            // list-element var(0) is tied in constrain_var_kernel.
            K::LogInfoWith | K::LogDebugWith | K::LogWarnWith | K::LogErrorWith => {
                fun(string(), fun(list(var(0)), task_unit()))
            }

            // ── Maybe ──
            K::MaybeWithDefault => fun(var(0), fun(maybe(var(0)), var(0))),
            K::MaybeMap => fun(fun(var(0), var(1)), fun(maybe(var(0)), maybe(var(1)))),
            K::MaybeAndThen => fun(
                fun(var(0), maybe(var(1))),
                fun(maybe(var(0)), maybe(var(1))),
            ),
            // `map2 : (a -> b -> v) -> Maybe a -> Maybe b -> Maybe v`. The N-ary
            // function is CURRIED at the Ipê type level (`a -> b -> v`); the
            // backend passes the multi-arg Rust fn value directly (mirrors
            // JsonDec.map2). var(0)=a, var(1)=b, .., last=v.
            K::MaybeMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(maybe(var(0)), fun(maybe(var(1)), maybe(var(2)))),
            ),
            K::MaybeMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(
                    maybe(var(0)),
                    fun(maybe(var(1)), fun(maybe(var(2)), maybe(var(3)))),
                ),
            ),
            K::MaybeMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    maybe(var(0)),
                    fun(
                        maybe(var(1)),
                        fun(maybe(var(2)), fun(maybe(var(3)), maybe(var(4)))),
                    ),
                ),
            ),
            K::MaybeMap5 => fun(
                fun(
                    var(0),
                    fun(var(1), fun(var(2), fun(var(3), fun(var(4), var(5))))),
                ),
                fun(
                    maybe(var(0)),
                    fun(
                        maybe(var(1)),
                        fun(
                            maybe(var(2)),
                            fun(maybe(var(3)), fun(maybe(var(4)), maybe(var(5)))),
                        ),
                    ),
                ),
            ),
            // `andMap : Maybe a -> Maybe (a -> b) -> Maybe b`. var(0)=a, var(1)=b.
            K::MaybeAndMap => fun(
                maybe(var(0)),
                fun(maybe(fun(var(0), var(1))), maybe(var(1))),
            ),
            // `combine : List (Maybe a) -> Maybe (List a)`. var(0)=a.
            K::MaybeCombine => fun(list(maybe(var(0))), maybe(list(var(0)))),
            // `isJust : Maybe a -> Bool`. var(0)=a.
            K::MaybeIsJust => fun(maybe(var(0)), bool_ty()),
            // `isNothing : Maybe a -> Bool`. var(0)=a.
            K::MaybeIsNothing => fun(maybe(var(0)), bool_ty()),

            // ── Result ──
            K::ResultWithDefault => fun(var(0), fun(result(var(1), var(0)), var(0))),
            K::ResultMap => fun(
                fun(var(0), var(1)),
                fun(result(var(2), var(0)), result(var(2), var(1))),
            ),
            // `andThen : (a -> Result e b) -> Result e a -> Result e b`.
            // var(0)=a, var(1)=e, var(2)=b. The error channel `e` is shared
            // across the callback's Result, the input Result, and the output.
            K::ResultAndThen => fun(
                fun(var(0), result(var(1), var(2))),
                fun(result(var(1), var(0)), result(var(1), var(2))),
            ),
            // `mapError : (e -> f) -> Result e a -> Result f a`.
            // var(0)=e, var(1)=f, var(2)=a. Maps the error channel; the `Ok`
            // value type `a` is preserved.
            K::ResultMapError => fun(
                fun(var(0), var(1)),
                fun(result(var(0), var(2)), result(var(1), var(2))),
            ),
            // `map2 : (a -> b -> v) -> Result e a -> Result e b -> Result e v`.
            // The error channel `e` is SHARED across all input `Result`s and the
            // output. var(0)=a, var(1)=b, var(2)=v, last var = e (shared).
            K::ResultMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(
                    result(var(3), var(0)),
                    fun(result(var(3), var(1)), result(var(3), var(2))),
                ),
            ),
            K::ResultMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(
                    result(var(4), var(0)),
                    fun(
                        result(var(4), var(1)),
                        fun(result(var(4), var(2)), result(var(4), var(3))),
                    ),
                ),
            ),
            K::ResultMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    result(var(5), var(0)),
                    fun(
                        result(var(5), var(1)),
                        fun(
                            result(var(5), var(2)),
                            fun(result(var(5), var(3)), result(var(5), var(4))),
                        ),
                    ),
                ),
            ),
            K::ResultMap5 => fun(
                fun(
                    var(0),
                    fun(var(1), fun(var(2), fun(var(3), fun(var(4), var(5))))),
                ),
                fun(
                    result(var(6), var(0)),
                    fun(
                        result(var(6), var(1)),
                        fun(
                            result(var(6), var(2)),
                            fun(
                                result(var(6), var(3)),
                                fun(result(var(6), var(4)), result(var(6), var(5))),
                            ),
                        ),
                    ),
                ),
            ),
            // `andMap : Result e a -> Result e (a -> b) -> Result e b`.
            // var(0)=a, var(1)=b, var(2)=e (shared).
            K::ResultAndMap => fun(
                result(var(2), var(0)),
                fun(
                    result(var(2), fun(var(0), var(1))),
                    result(var(2), var(1)),
                ),
            ),
            // `combine : List (Result e a) -> Result e (List a)`.
            // var(0)=a, var(1)=e.
            K::ResultCombine => fun(
                list(result(var(1), var(0))),
                result(var(1), list(var(0))),
            ),
            // `traverse : (a -> Result e b) -> List a -> Result e (List b)`.
            // var(0)=a, var(1)=b, var(2)=e.
            K::ResultTraverse => fun(
                fun(var(0), result(var(2), var(1))),
                fun(list(var(0)), result(var(2), list(var(1)))),
            ),
            // `toMaybe : Result e a -> Maybe a`. var(0)=e, var(1)=a.
            K::ResultToMaybe => fun(result(var(0), var(1)), maybe(var(1))),
            // `fromMaybe : e -> Maybe a -> Result e a`. var(0)=e, var(1)=a.
            K::ResultFromMaybe => fun(var(0), fun(maybe(var(1)), result(var(0), var(1)))),

            // ── Bytes ──
            K::BytesToString => fun(bytes(), maybe(string())),
            K::BytesFromHex | K::BytesFromBase64 => fun(string(), maybe(bytes())),

            // ── Task ──
            K::TaskSucceed => fun(var(0), task(var(0))),
            K::TaskFail => fun(error_ty(), task(var(0))),
            K::TaskMap => fun(fun(var(0), var(1)), fun(task(var(0)), task(var(1)))),
            // `Task.map2 : (a -> b -> r) -> Task Error a -> Task Error b -> Task Error r`.
            K::TaskMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(task(var(0)), fun(task(var(1)), task(var(2)))),
            ),
            K::TaskMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(
                    task(var(0)),
                    fun(task(var(1)), fun(task(var(2)), task(var(3)))),
                ),
            ),
            K::TaskMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    task(var(0)),
                    fun(
                        task(var(1)),
                        fun(task(var(2)), fun(task(var(3)), task(var(4)))),
                    ),
                ),
            ),
            K::TaskMap5 => fun(
                fun(
                    var(0),
                    fun(var(1), fun(var(2), fun(var(3), fun(var(4), var(5))))),
                ),
                fun(
                    task(var(0)),
                    fun(
                        task(var(1)),
                        fun(
                            task(var(2)),
                            fun(task(var(3)), fun(task(var(4)), task(var(5)))),
                        ),
                    ),
                ),
            ),
            // `Task.attempt : (Result Error a -> msg) -> Task Error a -> Cmd msg`.
            // var(0)=a, var(1)=msg. Mirrors `Cmd.perform` with args reordered.
            K::TaskAttempt => fun(
                fun(result(error_ty(), var(0)), var(1)),
                fun(task(var(0)), cmd(var(1))),
            ),
            K::TaskAndThen => fun(fun(var(0), task(var(1))), fun(task(var(0)), task(var(1)))),
            K::TaskMapError => fun(fun(error_ty(), error_ty()), fun(task(var(0)), task(var(0)))),
            K::TaskOnError => fun(
                fun(error_ty(), task(var(0))),
                fun(task(var(0)), task(var(0))),
            ),
            K::TaskFromResult => fun(result(var(0), var(1)), task(var(1))),
            K::TaskAndThenResult => fun(
                fun(var(0), result(var(1), var(2))),
                fun(task(var(0)), task(var(2))),
            ),
            K::TaskSequence | K::TaskParallel => fun(list(task(var(0))), task(list(var(0)))),
            // `Task.run : Task Error a -> Result Error a`.
            // The error channel is the fixed `Error` type — using `var(1)` here
            // leaves the result's error type free, causing IPE-L0102 at the
            // `main` binding in programs that end with `|> Task.run` and have no
            // annotation that would pin `var(1)` to `Error`.
            K::TaskRun => fun(task(var(0)), result(error_ty(), var(0))),
            // `Task.perform` is a 1-arg legacy alias for `Task.run`; identical type.
            K::TaskPerform => fun(task(var(0)), result(error_ty(), var(0))),
            // `Task.lazy : (() -> Task e a) -> Task e a`
            K::TaskLazy => fun(fun(Ty::Unit, task(var(0))), task(var(0))),
            // ── Task retry surface ──────────────────────────────────────────
            // `linearBackoff : Int -> Int -> RetryPolicy e`
            K::TaskLinearBackoff => fun(int(), fun(int(), retry_policy(var(0)))),
            // `exponentialBackoff : Int -> Int -> RetryPolicy e`
            K::TaskExponentialBackoff => fun(int(), fun(int(), retry_policy(var(0)))),
            // `withJitter : RetryPolicy e -> RetryPolicy e`
            K::TaskWithJitter => fun(retry_policy(var(0)), retry_policy(var(0))),
            // `retryOn / withRetryOn : (e -> Bool) -> RetryPolicy e -> RetryPolicy e`
            K::TaskRetryOn | K::TaskWithRetryOn => {
                fun(fun(var(0), bool_ty()), fun(retry_policy(var(0)), retry_policy(var(0))))
            }
            // `defaultRetryPolicy : RetryPolicy e`
            K::TaskDefaultRetryPolicy => retry_policy(var(0)),
            // `withMaxAttempts / withBaseMs : Int -> RetryPolicy e -> RetryPolicy e`
            K::TaskWithMaxAttempts | K::TaskWithBaseMs => {
                fun(int(), fun(retry_policy(var(0)), retry_policy(var(0))))
            }
            // `retryWith : RetryPolicy Error -> Task Error a -> Task Error a`
            K::TaskRetryWith => {
                fun(retry_policy(error_ty()), fun(task(var(0)), task(var(0))))
            }
            // `BackoffStrategy` nullary constructors.
            K::BackoffLinear
            | K::BackoffLinearWithJitter
            | K::BackoffExponential
            | K::BackoffExponentialWithJitter => backoff_strategy(),

            // ── Io / File / System: String -> Task () ──
            K::IoWriteStdout
            | K::IoWriteStderr
            | K::IoPrintln
            | K::IoEprintln
            | K::SystemUnsetenv => fun(string(), task_unit()),
            // File path-consuming `Path -> Task ()` kernels (typed path, not
            // a raw `String` — construction is the validated boundary).
            K::FileRemove | K::FileMkdirAll | K::FileDelete => fun(path(), task_unit()),
            // () -> Task String
            K::IoReadLine | K::SystemCwd | K::SystemGetcwd => fun(Ty::Unit, task(string())),
            // prompt String -> Task Secret (echo-suppressed line read). The line
            // is sealed into the opaque `Secret` at the read boundary — never a
            // bare `String` — so a freshly-read password cannot flow into a log,
            // error, or serialized payload by accident; consume it via
            // `Secret.use` / `Secret.reveal`.
            K::IoReadSecret => fun(string(), task(secret())),
            // ── Debug (dev-only) ──
            // `Debug.log : String -> a -> a`. BASE scheme only; the argument /
            // result share `var(0)`, which carries the STRINGIFY obligation
            // (`show`), tied in `constrain_var_kernel` (like `Log.*With`). A
            // production build rejects any use before this scheme is reached
            // (IPE-L0140, `reject_dev_only_kernels`).
            K::DebugLog => fun(string(), fun(var(0), var(0))),
            // `Debug.todo : String -> a` — diverging; result var is unconstrained
            // and unifies with any expected type, so a `todo` placeholder compiles
            // anywhere.  Reaching it at runtime aborts with a non-zero exit
            // (never a Rust panic). A `todo` in a `case` arm does NOT satisfy
            // exhaustiveness.
            K::DebugTodo => fun(string(), var(0)),
            // `Debug.explain : Attribute msg` — nullary; the message type var is
            // unconstrained.  Draws visible outlines on the element and all
            // descendants without changing layout.  Web/WebView only.
            K::DebugExplain => attr(var(0)),

            // ── Time ──
            K::TimeNow | K::TimeUnixMillis => fun(Ty::Unit, task(int())),
            K::TimeSleep => fun(int(), task_unit()),
            K::TimeEvery => fun(int(), fun(var(0), sub(var(0)))),

            // ── System ──
            // `getenv` takes an env-var NAME (String); `tempFile`/`tempDir`
            // take a filename PREFIX (String, sanitised in the runtime), so
            // these stay `String -> Task String` — they do not consume a path.
            K::SystemGetenv | K::FileTempFile | K::FileTempDir => fun(string(), task(string())),
            // `readFile` consumes a validated `Path`.
            K::FileReadFile => fun(path(), task(string())),
            K::SystemArgs => fun(Ty::Unit, task(list(string()))),
            K::SystemLoadEnv => fun(Ty::Unit, task_unit()),
            K::SystemSetenv => fun(string(), fun(string(), task_unit())),
            // `writeFile`/`append` take a `Path` then the content `String`.
            K::FileWriteFile | K::FileAppend => fun(path(), fun(string(), task_unit())),
            // `copy`/`rename` take two `Path`s (source then destination).
            K::FileCopy | K::FileRename => fun(path(), fun(path(), task_unit())),
            K::SystemGetArg => fun(int(), task(maybe(string()))),
            K::SystemGetenvInt => fun(string(), task(int())),
            K::SystemGetenvBool => fun(string(), task(bool_ty())),
            // `exists`/`isDir` query a validated `Path`.
            K::FileExists | K::FileIsDir => fun(path(), task(bool_ty())),
            K::SystemExit => fun(int(), var(0)),

            // ── Random ──
            K::RandomInt => fun(int(), fun(int(), task(int()))),
            K::RandomFloat => fun(float(), fun(float(), task(float()))),
            K::RandomChoice => fun(list(var(0)), task(var(0))),
            // choice : List a -> Task Error (Maybe a)   (total; Nothing when empty)
            K::RandomChoiceMaybe => fun(list(var(0)), task(maybe(var(0)))),
            // shuffle : List a -> Task Error (List a)
            K::RandomShuffle => fun(list(var(0)), task(list(var(0)))),
            // weighted : List (Float, a) -> Task Error (Maybe a)   (total)
            K::RandomWeighted => fun(list(tuple2(float(), var(0))), task(maybe(var(0)))),

            // ── Process ──
            // `run : String -> List String -> Task Error String`
            K::ProcessRun => fun(string(), fun(list(string()), task(string()))),
            // `runWith : { args, command, cwd, env } -> Task { exitCode, stderr, stdout }`
            // Input record fields in BTreeMap key order (ascending byte): args < command < cwd < env.
            // Output record fields in BTreeMap key order: exitCode < stderr < stdout.
            K::ProcessRunWith => {
                let mut input_fields = BTreeMap::new();
                input_fields.insert(self.builtins.process_f_args, list(string()));
                input_fields.insert(self.builtins.process_f_command, string());
                input_fields.insert(
                    self.builtins.process_f_cwd,
                    maybe(path()),
                );
                input_fields.insert(
                    self.builtins.process_f_env,
                    list(tuple2(string(), string())),
                );
                let input_rec = Ty::Record(input_fields, RowTail::Closed);

                let mut output_fields = BTreeMap::new();
                output_fields.insert(self.builtins.process_f_exit_code, int());
                output_fields.insert(self.builtins.process_f_stderr, string());
                output_fields.insert(self.builtins.process_f_stdout, string());
                let output_rec = Ty::Record(output_fields, RowTail::Closed);

                fun(input_rec, task(output_rec))
            }
            // `runInPty : { command, args, cwd, env, cols, rows } -> Task { exitCode, output }`
            // Records built via BTreeMap; iteration is by ascending symbol id.
            K::ProcessRunInPty => {
                let mut input_fields = BTreeMap::new();
                input_fields.insert(self.builtins.process_f_command, string());
                input_fields.insert(self.builtins.process_f_args, list(string()));
                input_fields.insert(self.builtins.process_f_cwd, maybe(path()));
                input_fields.insert(
                    self.builtins.process_f_env,
                    list(tuple2(string(), string())),
                );
                input_fields.insert(self.builtins.process_f_cols, int());
                input_fields.insert(self.builtins.process_f_rows, int());
                let input_rec = Ty::Record(input_fields, RowTail::Closed);

                let mut output_fields = BTreeMap::new();
                output_fields.insert(self.builtins.process_f_exit_code, int());
                output_fields.insert(self.builtins.process_f_output, string());
                let output_rec = Ty::Record(output_fields, RowTail::Closed);

                fun(input_rec, task(output_rec))
            }

            // ── File (remaining) — all consume a validated `Path` ──
            K::FileReadDir => fun(path(), task(list(string()))),
            K::FileReadFileLimit => fun(path(), fun(int(), task(string()))),
            K::FileReadFileBytes => fun(path(), task(list(int()))),
            // `walk : Path -> Task Error (List Path)` — recursive, files only.
            K::FileWalk => fun(path(), task(list(path()))),
            // `walkMatching : Path -> (Path -> Bool) -> Task Error (List Path)`
            K::FileWalkMatching => fun(path(), fun(fun(path(), bool_ty()), task(list(path())))),

            // ── Http ──
            K::HttpGet => fun(url(), task(http_response())),
            K::HttpPost => fun(url(), fun(string(), task(http_response()))),
            K::HttpRequest => fun(http_request(), task(http_response())),
            K::HttpParseQuery => fun(string(), dict(string(), string())),
            K::HttpDefaultRequest => fun(url(), result(error_ty(), http_request())),
            K::HttpDefaultRequestFromString => fun(string(), result(error_ty(), http_request())),
            K::HttpWithMethod => fun(http_method_ty(), fun(http_request(), http_request())),
            K::HttpMethodFromString => fun(string(), maybe(http_method_ty())),
            K::HttpMethodToString => fun(http_method_ty(), string()),
            K::HttpWithTimeout => fun(int(), fun(http_request(), http_request())),
            K::HttpWithBody => fun(string(), fun(http_request(), http_request())),
            K::HttpWithHeader => fun(string(), fun(string(), fun(http_request(), http_request()))),
            K::HttpWithUrl => fun(url(), fun(http_request(), result(error_ty(), http_request()))),
            K::HttpWithRedirects => fun(redirect_policy_ty(), fun(http_request(), http_request())),

            // ── Cmd ──
            K::CmdNone => cmd(var(0)),
            K::CmdBatch => fun(list(cmd(var(0))), cmd(var(0))),
            K::CmdPerform => fun(
                task(var(0)),
                fun(fun(result(error_ty(), var(0)), var(1)), cmd(var(1))),
            ),
            // `Cmd.map : (a -> msg) -> Cmd a -> Cmd msg` — retag a
            // sub-component's commands. var(0)=a (child msg), var(1)=msg (parent).
            K::CmdMap => fun(fun(var(0), var(1)), fun(cmd(var(0)), cmd(var(1)))),

            // ── Cmd.publish / Cmd.publishNoEcho ──
            // `Cmd.publish : Topic a -> a -> Cmd msg`
            // var(0) = msg, var(1) = payload type `a`
            K::CmdPublish => fun(topic(var(1)), fun(var(1), cmd(var(0)))),
            // `Cmd.publishNoEcho : Topic a -> a -> Cmd msg`
            K::CmdPublishNoEcho => fun(topic(var(1)), fun(var(1), cmd(var(0)))),

            // ── Sub ──
            K::SubNone => sub(var(0)),
            K::SubBatch => fun(list(sub(var(0))), sub(var(0))),
            K::SubEvery => fun(int(), fun(var(0), sub(var(0)))),
            // `Sub.map : (a -> msg) -> Sub a -> Sub msg` — the `Sub` twin of
            // `Cmd.map`. var(0)=a (child msg), var(1)=msg (parent).
            K::SubMap => fun(fun(var(0), var(1)), fun(sub(var(0)), sub(var(1)))),
            // `Sub.subscribeTopic : Topic a -> (a -> msg) -> Sub msg`
            // var(0) = msg, var(1) = payload type `a`
            K::SubSubscribeTopic => fun(topic(var(1)), fun(fun(var(1), var(0)), sub(var(0)))),

            // ── PubSub.publish / publishNoEcho ──
            // `PubSub.publish    : Topic a -> a -> Task Error Int`
            // `PubSub.publishNoEcho : Topic a -> a -> Task Error Int`
            // var(0) = payload type `a`.  Result is `Task Error Int` (subscriber
            // count), NOT `Cmd msg` — no `msg` type var, distinct from `Cmd.publish`.
            K::PubSubPublish => fun(topic(var(0)), fun(var(0), task(int()))),
            K::PubSubPublishNoEcho => fun(topic(var(0)), fun(var(0), task(int()))),

            // `PubSub.topic : String -> Topic a`
            // var(0) = payload type `a`
            K::PubSubTopic => fun(string(), topic(var(0))),

            // ── Server ──
            K::ServerGet
            | K::ServerPost
            | K::ServerPut
            | K::ServerDelete
            | K::ServerAny
            | K::ServerApi => fun(string(), fun(fun(req(), task(resp())), route())),
            K::ServerStatic => fun(string(), fun(string(), route())),
            // `mountApp : String -> WebApp -> Route`. The nominal `WebApp` in
            // the second slot is the §9 type gate: a `TuiApp`/`CliApp` (or any
            // non-`WebApp`) fails to unify here (IPE-T0001) — an app of the
            // wrong shape is unrepresentable in a mount.
            K::ServerMountApp => fun(string(), fun(web_app_leaf(), route())),
            K::ServerListen => fun(int(), fun(list(route()), task_unit())),
            K::ServerText | K::ServerJson | K::ServerHtml | K::ServerRedirect => {
                fun(string(), resp())
            }
            K::ServerWithStatus => fun(int(), fun(resp(), resp())),
            K::ServerWithHeader => fun(string(), fun(string(), fun(resp(), resp()))),
            K::ServerParam | K::ServerQueryParam | K::ServerHeader | K::ServerGetCookie => {
                fun(string(), fun(req(), maybe(string())))
            }
            K::ServerBody | K::ServerPath | K::ServerMethod => fun(req(), string()),
            K::ServerCookieNew => fun(string(), fun(string(), cookie())),
            K::ServerWithCookie => fun(cookie(), fun(resp(), resp())),
            // ── Authed routes (fail-closed Principal mint) ──
            // `authConfig : Secret -> TokenSource -> AuthConfig`.
            K::ServerAuthConfig => fun(secret(), fun(token_source(), auth_config())),
            // `bearerToken : TokenSource`; `cookieToken : String -> TokenSource`.
            K::ServerTokenBearer => token_source(),
            K::ServerCookieToken => fun(string(), token_source()),
            // `withRevocation : RevocationMode -> AuthConfig -> AuthConfig` — arms the gate;
            // web-server-level, same placement as `Server.authConfig`.
            K::ServerWithRevocation => fun(revocation_mode(), fun(auth_config(), auth_config())),
            // `getAuthed : String -> AuthConfig
            //     -> (Request -> Principal -> Task Error Response) -> Route`.
            K::ServerGetAuthed
            | K::ServerPostAuthed
            | K::ServerPutAuthed
            | K::ServerDeleteAuthed => fun(
                string(),
                fun(
                    auth_config(),
                    fun(fun(req(), fun(principal_ty(), task(resp()))), route()),
                ),
            ),

            // ── Middleware ──
            K::MiddlewareWithCors => fun(
                list(string()),
                fun(fun(req(), task(resp())), fun(req(), task(resp()))),
            ),
            K::MiddlewareWithLogging => fun(fun(req(), task(resp())), fun(req(), task(resp()))),
            K::MiddlewareWithBasicAuth => fun(
                string(),
                fun(
                    string(),
                    fun(fun(req(), task(resp())), fun(req(), task(resp()))),
                ),
            ),
            K::MiddlewareWithRateLimit => fun(
                string(),
                fun(
                    int(),
                    fun(
                        int(),
                        fun(fun(req(), task(resp())), fun(req(), task(resp()))),
                    ),
                ),
            ),
            K::MiddlewareWithCsrf => fun(fun(req(), task(resp())), fun(req(), task(resp()))),

            // ── Db ──
            K::DbConnect => fun(Ty::Unit, task(db())),
            K::DbOpen => fun(string(), fun(string(), task(db()))),
            K::DbClose => fun(db(), task_unit()),

            // ── Ipe.Db.Dsn — parse-don't-validate descriptor. ──
            // parse : String -> Result Error Dsn
            K::DsnParse => fun(string(), result(error_ty(), dsn())),
            // build : Int -> String -> Int -> String -> String -> Secret -> Int
            //   -> Result Error Dsn  (driverTag, host, port, database, user,
            //   password, tlsTag)
            K::DsnBuild => fun(
                int(),
                fun(
                    string(),
                    fun(
                        int(),
                        fun(
                            string(),
                            fun(
                                string(),
                                fun(secret(), fun(int(), result(error_ty(), dsn()))),
                            ),
                        ),
                    ),
                ),
            ),
            K::DsnDriverTag | K::DsnPort | K::DsnTlsTag => fun(dsn(), int()),
            K::DsnHost | K::DsnDatabase | K::DsnUser | K::DsnRedacted => fun(dsn(), string()),

            // ── External Connection — read-only-by-type foreign-DB connect. ──
            // open : Dsn -> Task Error (Connection ReadOnly)
            K::DbConnOpen => fun(dsn(), task(connection(conn_read_only()))),
            // close : Connection a -> Task Error ()  (polymorphic over the mode)
            K::DbConnClose => fun(connection(var(0)), task_unit()),
            // unsafeExecRawOn : Connection ReadWrite -> String -> Task Error Int
            K::DbConnUnsafeExecRawOn => {
                fun(connection(conn_read_write()), fun(string(), task(int())))
            }
            // ── External read path — mode-polymorphic `Connection a` first arg. ──
            // A read is available on any access mode, so the mode is a free var.
            // findWhereOn : Connection a -> String -> SqlFragment
            //               -> Task Error (List Row)
            K::DbConnFindWhere => fun(
                connection(var(0)),
                fun(
                    string(),
                    fun(sqlfragment(), task(list(dict(string(), string())))),
                ),
            ),
            // getByIdOn : Connection a -> String -> String
            //             -> Task Error (Maybe Row)
            K::DbConnGetById => fun(
                connection(var(0)),
                fun(
                    string(),
                    fun(string(), task(maybe(dict(string(), string())))),
                ),
            ),
            // queryDecodeOn : Connection a -> String -> List b -> Decoder c
            //                 -> Task Error (List c). var(2) = access mode (free,
            // never unifies with the decoder var(0) or the params-elem var(1)).
            K::DbConnQueryDecode => fun(
                connection(var(2)),
                fun(
                    string(),
                    fun(list(var(1)), fun(dec(var(0)), task(list(var(0))))),
                ),
            ),
            K::DbExecRaw => fun(db(), fun(string(), task(int()))),
            // `exec`/`query`/`queryDecode` accept `List a` (polymorphic) — any
            // Ipê type that can be bound as a SQL parameter: `List String`,
            // `List Int`, `List Float`, `List Bool`, or `List SqlValue` (typed
            // mixed-type binding).  The emitter routes all
            // three to `db_exec_params` / `db_query_params` /
            // `db_query_decode_params`, converting elements via
            // `ipe_runtime::db::SqlParam::from` which is implemented for every
            // Ipê-primitive type as well as for the generated `StdDbSqlValue`.
            K::DbExec => fun(db(), fun(string(), fun(list(var(0)), task(int())))),
            K::DbQuery => fun(
                db(),
                fun(
                    string(),
                    fun(list(var(0)), task(list(dict(string(), string())))),
                ),
            ),
            K::DbQueryDecode => fun(
                db(),
                fun(
                    string(),
                    // var(1) = element type of the params list (unconstrained);
                    // var(0) = decoder result type.
                    fun(list(var(1)), fun(dec(var(0)), task(list(var(0))))),
                ),
            ),
            K::DbGetString | K::DbGetField => {
                fun(string(), fun(dict(string(), string()), string()))
            }
            K::DbGetInt => fun(string(), fun(dict(string(), string()), int())),
            K::DbGetBool => fun(string(), fun(dict(string(), string()), bool_ty())),
            K::DbInsertRow => fun(
                db(),
                fun(string(), fun(dict(string(), string()), task(int()))),
            ),
            K::DbGetById => fun(
                db(),
                fun(
                    string(),
                    fun(string(), task(maybe(dict(string(), string())))),
                ),
            ),
            K::DbUpdateById => fun(
                db(),
                fun(
                    string(),
                    fun(string(), fun(dict(string(), string()), task(int()))),
                ),
            ),
            K::DbDeleteById => fun(db(), fun(string(), fun(string(), task(int())))),
            K::DbFindOneByField => fun(
                db(),
                fun(
                    string(),
                    fun(
                        string(),
                        fun(string(), task(maybe(dict(string(), string())))),
                    ),
                ),
            ),
            K::DbFindManyByField => fun(
                db(),
                fun(
                    string(),
                    fun(
                        string(),
                        fun(string(), task(list(dict(string(), string())))),
                    ),
                ),
            ),
            K::DbFindByConditions => fun(
                db(),
                fun(
                    string(),
                    fun(
                        dict(string(), string()),
                        task(list(dict(string(), string()))),
                    ),
                ),
            ),
            // `Db.findWhere : Db -> String -> SqlFragment -> Task (List Row)`
            // — the `SqlFragment`-typed replacement for the removed
            // `unsafeFindWhere`. A caller can never pass a raw
            // `String` WHERE clause here: only the `Sql.*` combinators below
            // produce a `SqlFragment`, so a naive string-concatenated WHERE
            // clause is a IPE-T0001 type mismatch, not a runtime risk.
            K::DbFindWhere => fun(
                db(),
                fun(string(), fun(sqlfragment(), task(list(dict(string(), string()))))),
            ),
            // `Db.findJoin : Db -> String -> String -> List String -> String
            //                -> String -> List String -> SqlFragment
            //                -> Task (List (Dict String String, Dict String String))`
            // — an inner join of two tables as one parameterized statement; each
            // result row is the pair of the two sides' plain-keyed cell maps, so
            // a caller decodes each side through its own store codec.
            K::DbFindJoin => fun(
                db(),
                fun(
                    string(),
                    fun(
                        string(),
                        fun(
                            list(string()),
                            fun(
                                string(),
                                fun(
                                    string(),
                                    fun(
                                        list(string()),
                                        fun(
                                            sqlfragment(),
                                            task(list(tuple2(
                                                dict(string(), string()),
                                                dict(string(), string()),
                                            ))),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
            // `Db.findProjection : Db -> String -> String -> String -> String
            //                      -> SqlFragment -> List (String, String, String)
            //                      -> Task (List (Dict String String))` — read a
            // typed projection over a two-table join as one parameterized
            // statement. The two `(table, alias)` pairs name the sides, `frag`
            // carries the join-key equality plus any filter, and the
            // `List ProjectionTerm` is the ordered projection descriptor list
            // (see `selectNamed`). Each result row is one cell map keyed by the
            // projection output names (`p0`, `p1`, …), decoded by position.
            // `Db.findProjection : Db -> String -> String -> String -> String
            //                      -> SqlFragment -> List ProjectionTerm
            //                      -> List SqlValue
            //                      -> Task (List (Dict String String))`
            // — `extraBinds` is `List SqlValue` (the generated per-project ADT);
            // schemed as `list(var(0))` so a concrete `SqlValue` element unifies.
            K::DbFindProjection => fun(
                db(),
                fun(
                    string(),
                    fun(
                        string(),
                        fun(
                            string(),
                            fun(
                                string(),
                                fun(
                                    sqlfragment(),
                                    fun(
                                        list(projection_term()),
                                        fun(
                                            list(var(0)),
                                            task(list(dict(string(), string()))),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
            // `Db.findJoinOrdered : Db -> String -> String -> List String -> String
            //                       -> String -> List String -> SqlFragment
            //                       -> String -> String -> Bool
            //                       -> Task (List (Dict String String, Dict String String))`
            // — ordered variant of `Db.findJoin` with three trailing args:
            // `orderAlias`, `orderCol`, and ascending `Bool`.
            K::DbFindJoinOrdered => fun(
                db(),
                fun(
                    string(),
                    fun(
                        string(),
                        fun(
                            list(string()),
                            fun(
                                string(),
                                fun(
                                    string(),
                                    fun(
                                        list(string()),
                                        fun(
                                            sqlfragment(),
                                            fun(
                                                string(),
                                                fun(
                                                    string(),
                                                    fun(
                                                        bool_ty(),
                                                        task(list(tuple2(
                                                            dict(string(), string()),
                                                            dict(string(), string()),
                                                        ))),
                                                    ),
                                                ),
                                            ),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
            // `Db.findProjectionOrdered : Db -> String -> String -> String -> String
            //                             -> SqlFragment -> List ProjectionTerm
            //                             -> List SqlValue
            //                             -> String -> String -> Bool
            //                             -> Task (List (Dict String String))`
            // — ordered variant of `Db.findProjection` with `extraBinds` (the
            // `Store.literal` bind list, schemed as `list(var(0))`) inserted
            // between `projections` and the three ORDER BY args.
            K::DbFindProjectionOrdered => fun(
                db(),
                fun(
                    string(),
                    fun(
                        string(),
                        fun(
                            string(),
                            fun(
                                string(),
                                fun(
                                    sqlfragment(),
                                    fun(
                                        list(projection_term()),
                                        fun(
                                            list(var(0)),
                                            fun(
                                                string(),
                                                fun(
                                                    string(),
                                                    fun(
                                                        bool_ty(),
                                                        task(list(dict(string(), string()))),
                                                    ),
                                                ),
                                            ),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
            // `Db.deleteWhere : Db -> String -> SqlFragment -> Task Int`
            K::DbDeleteWhere => fun(db(), fun(string(), fun(sqlfragment(), task(int())))),
            // `Db.updateWhere : Db -> String -> List (String, SqlField)
            //                   -> SqlFragment -> Task Int`
            K::DbUpdateWhere => fun(
                db(),
                fun(
                    string(),
                    fun(
                        list(tuple2(string(), sqlfield())),
                        fun(sqlfragment(), task(int())),
                    ),
                ),
            ),
            K::DbInsertFields => fun(
                db(),
                fun(
                    string(),
                    fun(list(tuple2(string(), sqlfield())), task(int())),
                ),
            ),
            K::DbUpdateFields => fun(
                db(),
                fun(
                    string(),
                    fun(
                        list(tuple2(string(), sqlvalue())),
                        fun(list(tuple2(string(), sqlfield())), task(int())),
                    ),
                ),
            ),
            K::DbInsertFieldsReturning => fun(
                db(),
                fun(
                    string(),
                    fun(
                        list(tuple2(string(), sqlfield())),
                        fun(string(), fun(dec(var(0)), task(list(var(0))))),
                    ),
                ),
            ),
            K::DbWithTransaction => fun(db(), fun(fun(db(), task(var(0))), task(var(0)))),
            // `Db.migrate : Db -> List Migration -> Task Error (List String)`.
            // The record-shaped `Migration` API is the surface; the
            // `db_migrate_apply` runtime kernel still takes `(name, sql)` pairs —
            // the emitter converts at the call site.
            K::DbMigrate => fun(
                db(),
                fun(list(migration()), task(list(string()))),
            ),
            // `Db.defaultMigration : String -> Migration` — a Migration named
            // with an empty SQL body.
            K::DbDefaultMigration => fun(string(), migration()),

            // `Store.join : Store a -> (a -> k) -> Store b -> (b -> k)
            //               -> Joined a b` — an inner join of two stores on key
            // equality. The two getter-arrow accessors name the join columns;
            // both share the key type `k` (var 2), so a mistyped join key is a
            // type mismatch. Lowering replaces each accessor with its validated
            // column and rewrites to the `joinNamed` stdlib helper.
            K::StoreJoin => fun(
                store(var(0)),
                fun(
                    fun(var(0), var(2)),
                    fun(
                        store(var(1)),
                        fun(fun(var(1), var(2)), joined(var(0), var(1))),
                    ),
                ),
            ),

            // `Store.select : (( a, b ) -> row) -> Joined a b -> Select row` —
            // project specific columns of a join. The lambda receives the two
            // sides' row records (`a` = var 0, `b` = var 1) so a field accessor
            // on a side (`author.name`) reads that side's real field type; its
            // result `row` (var 2) is the projected shape. The lambda is never
            // run over real rows — lowering reads the accessed column from a bare
            // `side.field` body and rewrites to the `selectNamed` stdlib helper,
            // failing closed (IPE-L0149) on any body that is not a single column
            // reference, so a computed projection cannot enter the SELECT.
            K::StoreSelect => fun(
                fun(tuple2(var(0), var(1)), var(2)),
                fun(joined(var(0), var(1)), select(var(2))),
            ),

            // `Store.literal : t -> t` — a projection-body element that binds its
            // argument as a SQL parameter (`? AS pN`) rather than naming a column.
            // The identity type scheme lets the projection lambda's return type
            // unify: `literal "x"` has type `String`, so
            // `(\(_, a) -> (literal "x", a.name))` has type `(String, String)`.
            // Recognized structurally at lowering (not emitted as a call).
            K::StoreLiteral => fun(var(0), var(0)),

            // `Store.upper : String -> String` — wraps a column reference in SQL
            // `UPPER(…)` inside a projection body. Recognized structurally at
            // lowering (not emitted as a runtime call); the inner argument must be
            // a direct `side.field` column accessor.
            K::StoreUpper => fun(string(), string()),
            // `Store.lower : String -> String` — symmetric counterpart wrapping
            // `LOWER(…)`. Same structural restrictions as `StoreUpper`.
            K::StoreLower => fun(string(), string()),
            // `Store.coalesce : Projection a -> Projection a -> Projection a` —
            // both operands share the same type variable; `a` is a scalar type
            // (`String`, `Int`, `Bool`, or `Float`). Recognized structurally at
            // lowering (not emitted as a runtime call).
            K::StoreCoalesce => fun(var(0), fun(var(0), var(0))),
            // `Store.add / .sub / .mul : number a => a -> a -> a` — arithmetic
            // over two numeric projection operands. Base scheme for the totality
            // gate; the Number obligation binding all three positions to one
            // bounded var is minted in `constrain_var_kernel`. Recognized
            // structurally at lowering (not emitted as a runtime call).
            K::StoreAdd | K::StoreSub | K::StoreMul => fun(var(0), fun(var(0), var(0))),

            // `Store.eq : (row -> t) -> t -> Cond` — the getter-arrow scheme lets
            // an accessor literal `.field` unify against the first parameter by
            // ordinary inference, pinning `t` to the field's type so the value's
            // type must match. Lowering replaces the accessor with the validated
            // column identifier and emits the `Compare` `Cond` constructor.
            K::StoreEqCol => fun(fun(var(0), var(1)), fun(var(1), cond(var(0)))),

            // `Store.eqBy : Codec t -> (row -> t) -> t -> Cond row` — the
            // enum/newtype accessor leaf. `Codec t` projects the value to a bound
            // `SqlValue`; the getter-arrow (as `StoreEqCol`) pins the column and
            // value types, and `row` threads to the `Cond` result.
            K::StoreEqBy => fun(
                codec(var(1)),
                fun(fun(var(0), var(1)), fun(var(1), cond(var(0)))),
            ),

            // Ordering and inequality comparison leaves — same getter-arrow scheme
            // as `StoreEqCol` / `StoreEqBy`.
            K::StoreNeqCol | K::StoreGtCol | K::StoreGteCol | K::StoreLtCol | K::StoreLteCol => {
                fun(fun(var(0), var(1)), fun(var(1), cond(var(0))))
            }
            K::StoreNeqBy | K::StoreGtBy | K::StoreGteBy | K::StoreLtBy | K::StoreLteBy => fun(
                codec(var(1)),
                fun(fun(var(0), var(1)), fun(var(1), cond(var(0)))),
            ),

            // `Store.like : (row -> String) -> String -> Cond row` — the accessor
            // must name a `String` field (pinned by the getter-arrow scheme); the
            // pattern is a plain `String` parameter.
            K::StoreLike => fun(fun(var(0), string()), fun(string(), cond(var(0)))),

            // `Store.isNull : (row -> t) -> Cond row` — arity 1, accessor only.
            // `Store.notNull : (row -> t) -> Cond row` — same shape.
            K::StoreIsNull | K::StoreNotNull => fun(fun(var(0), var(1)), cond(var(0))),

            // `Store.inList : (row -> t) -> List t -> Cond row` — the list carries
            // values of the same type as the accessor's field.
            K::StoreInListCol => {
                fun(fun(var(0), var(1)), fun(list(var(1)), cond(var(0))))
            }
            // `Store.inListBy : Codec t -> (row -> t) -> List t -> Cond row`.
            K::StoreInListBy => fun(
                codec(var(1)),
                fun(fun(var(0), var(1)), fun(list(var(1)), cond(var(0)))),
            ),

            // ── Db.Store schema-shaping builders (accessor-typed) ──
            // `primaryKey / serial / unique / defaultNow / touchOnUpdate :
            //   (row -> t) -> Draft row -> Draft row`
            // They refine a `Draft` (the unclassified table), before
            // classification. The getter-arrow scheme pins the accessor's source
            // type to the draft's row type. `t` (var 1) is the field type — it is
            // not constrained by the return type (any field may be a key/serial/…).
            K::StorePrimaryKey
            | K::StoreSerial
            | K::StoreUnique
            | K::StoreDefaultNow
            | K::StoreTouchOnUpdate => {
                fun(fun(var(0), var(1)), fun(draft(var(0)), draft(var(0))))
            }

            // `defaultText : (row -> String) -> String -> Draft row -> Draft row`
            // The accessor must name a `String` field (pinned by the getter-arrow
            // scheme). The default value is a plain `String` argument.
            K::StoreDefaultText => fun(
                fun(var(0), string()),
                fun(string(), fun(draft(var(0)), draft(var(0)))),
            ),

            // `defaultInt : (row -> Int) -> Int -> Draft row -> Draft row`
            // The accessor must name an `Int` field; the default value is `Int`.
            K::StoreDefaultInt => fun(
                fun(var(0), int()),
                fun(int(), fun(draft(var(0)), draft(var(0)))),
            ),

            // ── Db.Store row-security policy builders (accessor-typed) ──
            // `ownerColumn / immutable : (row -> t) -> Policy row`
            // The getter-arrow scheme pins the accessor's source type to the
            // policy's phantom `row`, which `secured` later unifies with the
            // store's row. `t` (var 1) is the field type — unconstrained by the
            // result (any field may name an owner or immutable column).
            K::StoreOwnerColumn | K::StoreImmutable => {
                fun(fun(var(0), var(1)), policy(var(0)))
            }

            // `orderByLeft : (a -> k) -> Order -> Joined a b -> Joined a b`
            // `orderByRight : (b -> k) -> Order -> Joined a b -> Joined a b`
            // The getter-arrow scheme pins the accessor's source type to the
            // relevant join side (`a` for left, `b` for right). `Order` is the
            // sort-direction ADT (`Asc | Desc`). The `Joined a b` threads through
            // unchanged — lowering attaches the ORDER BY fragment to the query
            // plan rather than producing a new type. `k` (var 2) is the accessor's
            // return type — unconstrained (any field may sort).
            K::StoreOrderByLeft => fun(
                fun(var(0), var(2)),
                fun(order(), fun(joined(var(0), var(1)), joined(var(0), var(1)))),
            ),
            K::StoreOrderByRight => fun(
                fun(var(1), var(2)),
                fun(order(), fun(joined(var(0), var(1)), joined(var(0), var(1)))),
            ),

            // ── Db.Decode ──
            K::DbDecString => fun(string(), dec(string())),
            K::DbDecInt => fun(string(), dec(int())),
            K::DbDecFloat => fun(string(), dec(float())),
            K::DbDecBool => fun(string(), dec(bool_ty())),
            K::DbDecFail => fun(string(), dec(var(0))),
            K::DbDecNullable => fun(dec(var(0)), dec(maybe(var(0)))),
            K::DbDecMap => fun(fun(var(0), var(1)), fun(dec(var(0)), dec(var(1)))),
            K::DbDecAndThen => fun(fun(var(0), dec(var(1))), fun(dec(var(0)), dec(var(1)))),
            K::DbDecSucceed => fun(var(0), dec(var(0))),
            K::DbDecMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dec(var(0)), fun(dec(var(1)), dec(var(2)))),
            ),
            K::DbDecMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(dec(var(0)), fun(dec(var(1)), fun(dec(var(2)), dec(var(3))))),
            ),
            K::DbDecMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    dec(var(0)),
                    fun(dec(var(1)), fun(dec(var(2)), fun(dec(var(3)), dec(var(4))))),
                ),
            ),
            K::DbDecRequired => fun(
                string(),
                fun(dec(var(0)), fun(dec(fun(var(0), var(1))), dec(var(1)))),
            ),
            K::DbDecOptional => fun(
                string(),
                fun(
                    dec(var(0)),
                    fun(var(0), fun(dec(fun(var(0), var(1))), dec(var(1)))),
                ),
            ),
            // `Db.Decode.money : String -> Decoder (Decimal, String)` — decodes a
            // `"ISO_CODE AMOUNT"` TEXT column (the lossless serialisation
            // `SqlMoney` writes on INSERT) back into its amount/currency-code
            // pair. Deliberately NOT `Decoder Money`: `Money`/`Currency` are
            // project-generated types unnameable from this crate (see
            // `docs/adr/0013-multi-driver-db-compile-time-selection.md`) — a
            // recorded divergence from the the backend's `Decoder Money`,
            // sanctioned divergence §B-DbDecMoney.
            K::DbDecMoney => fun(string(), dec(tuple2(decimal(), string()))),
            // `Db.Decode.decimal : String -> Decoder Decimal` — reads an exact-decimal
            // TEXT column. FIRST_SCHEMED (Ipê-new, no legacy oracle).
            K::DbDecDecimal => fun(string(), dec(decimal())),
            // `Db.Decode.bytes : String -> Decoder (List Int)` — hex-decodes a
            // BYTEA/BLOB column. Ipê's `Bytes`/`List Int` representation is a
            // `List Int`; the runtime returns `Vec<u8>` which lowers identically.
            // FIRST_SCHEMED (Ipê-new, no legacy oracle).
            K::DbDecBytes => fun(string(), dec(list(int()))),

            // ── Set (base schemes; the `set_elem` obligation is layered in
            //    constrain_var_kernel, keyed off the id) ──
            K::SetEmpty => set(var(0)),
            K::SetSize => fun(set(var(0)), int()),
            K::SetInsert | K::SetRemove => fun(var(0), fun(set(var(0)), set(var(0)))),
            K::SetMember => fun(var(0), fun(set(var(0)), bool_ty())),
            K::SetToList => fun(set(var(0)), list(var(0))),
            K::SetFromList => fun(list(var(0)), set(var(0))),
            K::SetUnion | K::SetIntersect | K::SetDiff => {
                fun(set(var(0)), fun(set(var(0)), set(var(0))))
            }
            // isEmpty : Set a -> Bool
            K::SetIsEmpty => fun(set(var(0)), bool_ty()),
            // singleton : a -> Set a
            K::SetSingleton => fun(var(0), set(var(0))),
            // foldl / foldr : (a -> b -> b) -> b -> Set a -> b
            K::SetFoldl | K::SetFoldr => fun(
                fun(var(0), fun(var(1), var(1))),
                fun(var(1), fun(set(var(0)), var(1))),
            ),
            // map : (a -> b) -> Set a -> Set b (var 0=a AND var 1=b carry the
            // set_elem Ord obligation, layered in constrain_var_kernel).
            K::SetMap => fun(fun(var(0), var(1)), fun(set(var(0)), set(var(1)))),
            // filter : (a -> Bool) -> Set a -> Set a
            K::SetFilter => fun(fun(var(0), bool_ty()), fun(set(var(0)), set(var(0)))),
            // partition : (a -> Bool) -> Set a -> (Set a, Set a)
            K::SetPartition => fun(
                fun(var(0), bool_ty()),
                fun(set(var(0)), tuple2(set(var(0)), set(var(0)))),
            ),

            // ── Dict (base schemes; the `dict_key` obligation is layered in
            //    constrain_var_kernel, keyed off the id) ──
            K::DictEmpty => dict(var(0), var(1)),
            K::DictIsEmpty => fun(dict(var(0), var(1)), bool_ty()),
            K::DictSize => fun(dict(var(0), var(1)), int()),
            K::DictInsert => fun(
                var(0),
                fun(var(1), fun(dict(var(0), var(1)), dict(var(0), var(1)))),
            ),
            K::DictGet => fun(var(0), fun(dict(var(0), var(1)), maybe(var(1)))),
            K::DictRemove => fun(var(0), fun(dict(var(0), var(1)), dict(var(0), var(1)))),
            K::DictMember => fun(var(0), fun(dict(var(0), var(1)), bool_ty())),
            K::DictKeys => fun(dict(var(0), var(1)), list(var(0))),
            K::DictValues => fun(dict(var(0), var(1)), list(var(1))),
            K::DictToList => fun(dict(var(0), var(1)), list(tuple2(var(0), var(1)))),
            K::DictFromList => fun(list(tuple2(var(0), var(1))), dict(var(0), var(1))),
            K::DictMap => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dict(var(0), var(1)), dict(var(0), var(2))),
            ),
            K::DictFoldl => fun(
                fun(var(0), fun(var(1), fun(var(2), var(2)))),
                fun(var(2), fun(dict(var(0), var(1)), var(2))),
            ),
            K::DictUnion => fun(
                dict(var(0), var(1)),
                fun(dict(var(0), var(1)), dict(var(0), var(1))),
            ),
            // singleton : k -> v -> Dict k v
            K::DictSingleton => fun(var(0), fun(var(1), dict(var(0), var(1)))),
            // foldr : (k -> v -> a -> a) -> a -> Dict k v -> a
            K::DictFoldr => fun(
                fun(var(0), fun(var(1), fun(var(2), var(2)))),
                fun(var(2), fun(dict(var(0), var(1)), var(2))),
            ),
            // filter : (k -> v -> Bool) -> Dict k v -> Dict k v
            K::DictFilter => fun(
                fun(var(0), fun(var(1), bool_ty())),
                fun(dict(var(0), var(1)), dict(var(0), var(1))),
            ),
            // partition : (k -> v -> Bool) -> Dict k v -> (Dict k v, Dict k v)
            K::DictPartition => fun(
                fun(var(0), fun(var(1), bool_ty())),
                fun(
                    dict(var(0), var(1)),
                    tuple2(dict(var(0), var(1)), dict(var(0), var(1))),
                ),
            ),
            // intersect / diff : Dict k v -> Dict k v -> Dict k v
            K::DictIntersect | K::DictDiff => fun(
                dict(var(0), var(1)),
                fun(dict(var(0), var(1)), dict(var(0), var(1))),
            ),
            // update : k -> (Maybe v -> Maybe v) -> Dict k v -> Dict k v
            K::DictUpdate => fun(
                var(0),
                fun(
                    fun(maybe(var(1)), maybe(var(1))),
                    fun(dict(var(0), var(1)), dict(var(0), var(1))),
                ),
            ),

            // ── Ipe.Ui layout / element / event (already schemed in kernel_ty) ──
            K::UiLayout => fun(list(attr(var(0))), fun(elem_t(var(0)), html_t(var(0)))),
            K::UiLayoutWith => {
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.lw_wrapper_attrs, list(attr(var(0))));
                    m.insert(self.builtins.lw_root_attrs, list(attr(var(0))));
                    m
                }, RowTail::Closed);
                fun(cfg_rec, fun(elem_t(var(0)), html_t(var(0))))
            }
            // `Ui.node : Description -> List (Attribute msg) -> List (Element msg) -> Element msg`
            // — the container-element primitive backing `el`/`row`/`column`/
            // `wrappedRow`/`grid` in `Ipe/Ui.ipe`.
            K::UiNode => fun(
                description(),
                fun(list(attr(var(0))), fun(list(elem_t(var(0))), elem_t(var(0)))),
            ),
            // `Ui.taggedNode : String -> Description -> List (Attribute msg) -> List (Element msg) -> Element msg`
            // — the tagged-element primitive backing `paragraph`/`textColumn`/
            // `form`/`input`.
            K::UiTaggedNode => fun(
                string(),
                fun(
                    description(),
                    fun(list(attr(var(0))), fun(list(elem_t(var(0))), elem_t(var(0)))),
                ),
            ),
            // ── Ipe.Ui nearby attribute builders ─────────────────────────────────
            // `Ui.above/below/onLeft/onRight/inFront/behind : Element msg -> Attribute msg`
            K::UiAbove
            | K::UiBelow
            | K::UiOnLeft
            | K::UiOnRight
            | K::UiInFront
            | K::UiBehind => fun(elem_t(var(0)), attr(var(0))),
            K::UiButton => {
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.btn_f_on_press, maybe(var(0)));
                    m.insert(self.builtins.btn_f_label, elem_t(var(0)));
                    m
                }, RowTail::Closed);
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }
            K::UiOnClick | K::UiOnFocus | K::UiOnBlur | K::UiOnMouseOver | K::UiOnMouseOut => {
                fun(var(0), attr(var(0)))
            }
            K::UiOnInput | K::UiOnChange | K::UiOnKeyDown | K::UiOnKeyUp | K::UiOnFile => {
                fun(fun(string(), var(0)), attr(var(0)))
            }
            K::UiOnBool => fun(fun(bool_ty(), var(0)), attr(var(0))),
            // `Ui.onSubmit : (a -> msg) -> Attribute msg`
            // var(1) = the form-data record type (decoupled from var(0) = msg)
            K::UiOnSubmit => fun(fun(var(1), var(0)), attr(var(0))),

            // ── Ipe.Html.Events builders — produce `Ipe.Html.Attribute
            // msg` (`html_attr`), matching the `Ipe.Html.Attributes` builders
            // and the element builders' `List (html_attr msg)` slot. The arg
            // shape is dictated by `html_event_shape`; the `Raw` (onSubmit) form
            // DECOUPLES the handler type (`var(1)`) from `msg` (`var(0)`) so a
            // form handler `LoginForm -> Msg` does not leak into the surrounding
            // `Html msg` — exactly as the `.ipe` `onSubmit : a -> Attribute msg`.
            K::HtmlOnClick
            | K::HtmlOnFocus
            | K::HtmlOnBlur
            | K::HtmlOnMouseOver
            | K::HtmlOnMouseOut
            | K::HtmlOnSubmit
            | K::HtmlOnInput
            | K::HtmlOnChange
            | K::HtmlOnKeyDown
            | K::HtmlOnKeyUp
            | K::HtmlOnBool => match k.html_event_shape()? {
                ipe_kernels::HtmlEventShape::Msg => fun(var(0), html_attr(var(0))),
                ipe_kernels::HtmlEventShape::String => {
                    fun(fun(string(), var(0)), html_attr(var(0)))
                }
                ipe_kernels::HtmlEventShape::Bool => fun(fun(bool_ty(), var(0)), html_attr(var(0))),
                // `onSubmit : a -> Attribute msg` — the handler `var(1)` stays
                // an unconstrained HM var here (Ipê-level polymorphism only —
                // see `html.rs`'s `Event::OnForm` for the runtime-typed
                // construction, not `Event::OnRaw`, which no longer exists).
                ipe_kernels::HtmlEventShape::Raw => fun(var(1), html_attr(var(0))),
            },

            // ── Ipe.Web app-entry (open 6-field scheme) ──
            //
            // Mirrors `../ipe/src/Ipe/Type/Constrain/Expression.hs:2674-2695`.
            // The cfg record is OPEN (row variable `var(3)` = `appExt`) so the
            // user can supply optional extra fields (`head`, `consoleAuth`,
            // `guard`, `status`, `auth`, …) without the type checker rejecting
            // them as unknown extras.  The six named fields (indices 0-5) are
            // the REQUIRED fields; the row variable absorbs all additional ones.
            //
            // var index mapping:
            //   var(0) = model      var(1) = msg
            //   var(2) = page       var(3) = appExt (open row tail)
            //
            // `routes : List WebRoute` and `notFound : page` are required fields
            // even for non-routed apps (they default to `[]` / `CounterPage`).
            // The emit stage branches on Model.page at code-gen time
            // (emit_web.rs T5) — not at type time.
            //
            // Removes #[allow(dead_code)] from `live_f_routes` / `live_f_not_found`.
            // `Web.embed` shares `Web.app`'s exact six-field cfg scheme — both
            // produce the `WebApp` leaf from the same record. `embed`'s handle
            // is destined for `Server.mountApp`; `app`'s binds its own listener.
            K::WebApp | K::WebEmbed => {
                // `view : Model -> Element Msg`; the framework applies
                // `Ui.layout` internally, unifying the graphical shapes on
                // `Element`. Raw HTML is reached through the `Ui.html` node
                // inside this single `Element` view.
                let view_ret = elem_t(var(1));
                let init_ret = tuple2(var(0), cmd(var(1)));
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.live_f_init, fun(web_req(), init_ret.clone()));
                        m.insert(
                            self.builtins.live_f_update,
                            fun(var(1), fun(var(0), init_ret)),
                        );
                        m.insert(self.builtins.live_f_view, fun(var(0), view_ret));
                        m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                        // routes : List (WebRoute page)  — page = var(2).
                        // Parametrising WebRoute on the page type variable
                        // connects each route ctor's page type to `notFound`'s
                        // page type through the SAME var(2), so a type mismatch
                        // between them is caught here (IPE-T0001) instead of
                        // passing ipe and failing later in cargo (E0308).
                        m.insert(self.builtins.live_f_routes, list(live_route(var(2))));
                        // notFound : page
                        m.insert(self.builtins.live_f_not_found, var(2));
                        m
                    },
                    // Open row tail — var(3) absorbs optional extra fields.
                    RowTail::Open(3),
                );
                fun(cfg_rec, web_app_leaf())
            }
            // `Web.appWith : List (Setting Web) -> { … } -> WebApp` — the
            // additive settings-carrying web entry. Same cfg record as
            // `K::WebApp`, preceded by a shape-pinned `List (Setting Web)`: a
            // `Terminal`-only or cross-shape setting in that slot is an
            // IPE-T0001 type error, never a silently-ignored setting.
            K::WebAppWith => {
                let view_ret = elem_t(var(1));
                let init_ret = tuple2(var(0), cmd(var(1)));
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.live_f_init, fun(web_req(), init_ret.clone()));
                        m.insert(
                            self.builtins.live_f_update,
                            fun(var(1), fun(var(0), init_ret)),
                        );
                        m.insert(self.builtins.live_f_view, fun(var(0), view_ret));
                        m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                        m.insert(self.builtins.live_f_routes, list(live_route(var(2))));
                        m.insert(self.builtins.live_f_not_found, var(2));
                        m
                    },
                    RowTail::Open(3),
                );
                fun(list(setting(shape_web())), fun(cfg_rec, web_app_leaf()))
            }
            // `Web.route : String -> builder -> WebRoute page`
            // with builder = var(1) DISTINCT from page = var(0).
            //
            // The second argument is either a nullary page VALUE
            // (`route "/" HomePage` — builder : Page) or a params-consuming
            // page CONSTRUCTOR (`route "/apps/:slug" AppPage` — builder :
            // String -> Page; multi-`:param` routes curry further).  Sharing
            // ONE variable for both (the pre-round-4 `fun(var(0),
            // live_route(var(0)))` shape) forced `Page ≟ String -> Page` on
            // every param route — a false IPE-T0001 on the CANONICAL corpus
            // shape (`route "/apps/:slug" AppDetailPage`).
            //
            // Instead the builder var is related to the page var by a deferred
            // per-route witness ([`RouteWitnessCheck`], pushed in the
            // `constrain_kernel` special-case below and discharged by
            // `crate::resolve_route_witness_checks` after the main solve):
            // peel the builder's settled leading arrows, then unify the result
            // with `page`.  A nullary route witnesses `page` directly; a param
            // ctor witnesses it with its RESULT type; a wrong-ADT ctor still
            // fails unification → IPE-T0001.
            //
            // The result `WebRoute page` places every route of a list in
            // `List (WebRoute var(2))` (K::WebApp scheme), so all routes AND
            // `notFound : var(2)` share one page variable.  The page arg is no
            // longer phantom at the IR level: the lowerer threads it into
            // `IrType::WebRoute(page)` and the backend renders `Route<Page>`.
            K::WebRoute => fun(string(), fun(var(1), live_route(var(0)))),
            K::WebRenderStatic => fun(fun(var(0), html_t(var(1))), fun(var(0), task_unit())),

            // ── Ipe.Terminal full-screen app-entry (`Tui.app`) ────────────────
            //
            // `view : Model -> Cells Msg`, driven by `onKey`. `onKey` is
            // REQUIRED because the runtime's `tui_app_ui` entry takes a concrete
            // `FOnKey: Fn(String, String) -> Msg` bound (no `Option` form), so a
            // `Msg` cannot be fabricated when the handler is absent.
            //
            // Variable assignment:
            //   var(0) = model
            //   var(1) = msg
            //   var(3) = appExt     (open-row tail, absorbs guard/canvasWidth/…)
            //
            // `onKey`'s parameter is PINNED to the closed record
            // `{ kind : String, value : String }` (the KeyEvent shape): the
            // emitted handler must satisfy the runtime's
            // `FOnKey: Fn(String, String) -> Msg` bound, so an unconstrained
            // param would type-check yet break `cargo build` (E0593).
            K::TerminalAppScreen => {
                let key_event = Ty::Record(
                    {
                        let mut k = BTreeMap::new();
                        k.insert(self.builtins.tui_f_key_kind, string());
                        k.insert(self.builtins.tui_f_key_value, string());
                        k
                    },
                    RowTail::Closed,
                );
                let tup = tuple2(var(0), cmd(var(1)));
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.live_f_init, fun(Ty::Unit, tup.clone()));
                        m.insert(self.builtins.live_f_update, fun(var(1), fun(var(0), tup)));
                        m.insert(self.builtins.live_f_view, fun(var(0), cells_t(var(1))));
                        m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                        // onKey : { kind : String, value : String } -> msg (pinned).
                        m.insert(self.builtins.tui_f_on_key, fun(key_event, var(1)));
                        m
                    },
                    // Open row: absorbs optional fields (guard, canvasWidth, canvasHeight, …).
                    RowTail::Open(3),
                );
                fun(cfg_rec, tui_app_leaf())
            }

            // ── Ipe.Terminal line-oriented app-entry (`Cli.app`) ───────────────
            // `Cli.app : { init : () -> (model, Cmd msg)
            //                      , update : msg -> model -> (model, Cmd msg)
            //                      , view : model -> String
            //                      , subscriptions : model -> Sub msg
            //                      , onLine : String -> msg
            //                      } -> Task () ()`
            K::TerminalAppLines => {
                let tup = tuple2(var(0), cmd(var(1)));
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.live_f_init, fun(Ty::Unit, tup.clone()));
                        m.insert(self.builtins.live_f_update, fun(var(1), fun(var(0), tup)));
                        m.insert(self.builtins.live_f_view, fun(var(0), string()));
                        m.insert(self.builtins.live_f_subscriptions, fun(var(0), sub(var(1))));
                        m.insert(self.builtins.cli_f_on_line, fun(string(), var(1)));
                        m
                    },
                    // Closed cfg record — like `Tui.app`, the line cfg takes
                    // exactly its named fields (the open row is a `Web.app`-only
                    // surface).
                    RowTail::Closed,
                );
                fun(cfg_rec, cli_app_leaf())
            }

            // ══ FIRST-SCHEMED families ══
            // These have NO legacy scheme (`kernel_ty` → `Ty::Var(u32::MAX)`
            // hole); they get their scheme here, authored from the runtime
            // signature + `.ipe` HM signature. No parity oracle exists, so
            // correctness is pinned by `first_schemed_were_holes` (each is a
            // genuine hole) plus ipe→cargo build fixtures. Every arrow-count
            // equals `decl().arity` — the invariant
            // `eta_expand_partial` relies on when peeling `arity` arrows off the
            // inferred callee type.

            // ── String (33 — the kernels beyond `fromInt`/`fromFloat`) ──
            K::StringToInt => fun(string(), maybe(int())),
            K::StringToFloat => fun(string(), maybe(float())),
            K::StringFromList => fun(list(char()), string()),
            K::StringConcat => fun(list(string()), string()),
            K::StringWords | K::StringLines => fun(string(), list(string())),
            K::StringToList => fun(string(), list(char())),
            K::StringJoin => fun(string(), fun(list(string()), string())),
            K::StringSplit => fun(string(), fun(string(), list(string()))),
            // uncons : String -> Maybe (Char, String)
            K::StringUncons => fun(string(), maybe(tuple2(char(), string()))),
            // indexes : String -> String -> List Int
            K::StringIndexes => fun(string(), fun(string(), list(int()))),
            // foldl / foldr : (Char -> b -> b) -> b -> String -> b
            K::StringFoldl | K::StringFoldr => fun(
                fun(char(), fun(var(0), var(0))),
                fun(var(0), fun(string(), var(0))),
            ),

            // ── Crypto AEAD / Result-returning arms (the monomorphic hash /
            //    HMAC / verify kernels carry a shape; these keep a table arm for
            //    their `Result`/`Task` return). AEAD `decl().arity` is 2 (a fresh
            //    random nonce is prepended internally by the runtime). ──
            //    registry `decl().arity` was corrected 3→2 to match the Rust
            //    runtime (`ipe_aes_gcm_encrypt(key, plaintext)` — a fresh random
            //    nonce is prepended internally, so no third arg). Both take
            //    `key -> plaintext/ciphertext -> Result Error String`. ──
            K::CryptoRsaSha256Sign => {
                fun(string(), fun(string(), result(error_ty(), string())))
            }
            // AEAD requires a typed `Key` in the key role — a bare `String`
            // (message/plaintext) can never stand in for the key.
            K::CryptoAesGcmEncrypt
            | K::CryptoAesGcmDecrypt
            | K::CryptoChacha20Encrypt
            | K::CryptoChacha20Decrypt => {
                fun(crypto_key(), fun(string(), result(error_ty(), string())))
            }
            // Key-derivation returns a typed `Key`, never a raw `String`, so a
            // derived key can only flow into a `Key`-typed sink.
            K::CryptoAesKeyFromPassword | K::CryptoChachaKeyFromPassword => {
                fun(string(), fun(string(), crypto_key()))
            }
            K::CryptoRandomBytes | K::CryptoRandomToken => fun(int(), task(string())),

            // ── Jwt (4) — `secret -> token/claims -> Result Error String`.
            //    Decode returns the decoded claims JSON as a String; encode
            //    (`ipe_jwt_encode_hs256(secret, claims_json)`) takes the secret/
            //    key and a claims-JSON String and returns the signed token — the
            //    registry `decl().arity` was corrected 3→2 to match. ──
            K::JwtDecodeHs256 | K::JwtDecodeRs256 | K::JwtEncodeHs256 | K::JwtEncodeRs256 => {
                fun(string(), fun(string(), result(error_ty(), string())))
            }

            // ── Jwt builder API (D-00) ──────────────────────────────────
            // `Jwt.claims : Claims` — nullary: returns an empty claims object.
            K::JwtClaims => claims_ty(),
            // `Jwt.hs256 : String -> Algorithm`
            // `Jwt.rs256 : String -> Algorithm`
            K::JwtHs256 | K::JwtRs256 => fun(string(), algorithm_ty()),
            // `Jwt.subject / .issuer / .audience / .jwtId : String -> Claims -> Claims`
            K::JwtSubject | K::JwtIssuer | K::JwtAudience | K::JwtJwtId => {
                fun(string(), fun(claims_ty(), claims_ty()))
            }
            // `Jwt.expiresAt / .notBefore / .issuedAt : Int -> Claims -> Claims`
            K::JwtExpiresAt | K::JwtNotBefore | K::JwtIssuedAt => {
                fun(int(), fun(claims_ty(), claims_ty()))
            }
            // `Jwt.withClaim : String -> JsonEnc.Value -> Claims -> Claims`
            // Matches the reference `Ipê/Core/Jwt.ipe:79` — the value is any
            // encoded JSON node (`JsonEnc.string`/`.int`/`.object`/…), so an
            // `Int`/`Bool`/nested-object custom claim round-trips with the right
            // token bytes. Both `Value` and `Claims` are `serde_json::Value` at
            // runtime, so the runtime insert is a direct move.
            K::JwtWithClaim => fun(string(), fun(value(), fun(claims_ty(), claims_ty()))),
            // `Jwt.encode : Algorithm -> Claims -> Result Error String`
            K::JwtEncode => fun(algorithm_ty(), fun(claims_ty(), result(error_ty(), string()))),
            // `Jwt.decode : Algorithm -> Int -> String -> Result Error String`
            K::JwtDecode => fun(algorithm_ty(), fun(int(), fun(string(), result(error_ty(), string())))),

            // ── Encoding decoders — `String -> Result Error String` (decoded
            //    bytes must be valid UTF-8 — non-UTF-8 payloads surface as `Err`;
            //    raw bytes go through `Ipe.Bytes`). The `String -> String`
            //    encoders carry a shape and resolve via `resolve_scheme`. ──
            K::EncodingBase64Decode | K::EncodingUrlDecode | K::EncodingHexDecode => {
                fun(string(), result(error_ty(), string()))
            }

            // ── Ipe.Html / Ipe.Ui / Ipe.Web rendering (42) ──
            // The Html/Ui/Background/Border/Font rendering family. `attr(m)` /
            // `elem_t(m)` / `html_t(m)` are the msg-polymorphic opaque cons;
            // `length()` / `color()` are the nullary value cons. Each is a
            // genuine `Ty::Var(u32::MAX)` hole (legacy `kernel_ty` has no Html/
            // Ui/Background/Border/Font arm), so all land in FIRST_SCHEMED.
            // Verified vs runtime fn params + lower `callee_arity` per
            // docs/adr/0020-html-ui-live-kernel-arity-tripwire.md. `Web.appRouted`
            // is EXCLUDED (REACHABLE_BUT_UNLOWERED) — its lowering is
            // `Feature::RoutedWebApp` unsupported, so a caller fails closed.

            // Ipe.Html serialise / escape (arity 1).
            K::HtmlRender => fun(html_t(var(0)), string()),
            K::HtmlAttrToString => fun(html_attr(var(0)), string()),

            // Ipe.Ui element builders (arity 0 / 1).
            K::UiNone => elem_t(var(0)),
            K::UiText => fun(string(), elem_t(var(0))),
            K::UiHtml => fun(html_t(var(0)), elem_t(var(0))),
            K::UiCells => fun(list(list(char())), elem_t(var(0))),
            // Ipe.Ui.Cells Cells-typed builders.
            K::UiCellsNone => cells_t(var(0)),
            K::UiCellsText => fun(string(), cells_t(var(0))),
            K::UiCellsCells => fun(list(list(char())), cells_t(var(0))),
            // `Tui.Ui.el : List (Attribute msg) -> Screen msg -> Screen msg`
            // (the cell-native `tui_attr`, NOT the DOM `attr`).
            K::UiCellsEl => fun(
                list(tui_attr(var(0))),
                fun(cells_t(var(0)), cells_t(var(0))),
            ),
            // `Tui.Ui.row/column : List (Attribute msg) -> List (Screen msg) -> Screen msg`
            K::UiCellsRow | K::UiCellsColumn => fun(
                list(tui_attr(var(0))),
                fun(list(cells_t(var(0))), cells_t(var(0))),
            ),
            // ── Ipe.Tea.Tui.Ui cell-native attribute builders ──
            K::TuiUiSpacing | K::TuiUiPadding => fun(int(), tui_attr(var(0))),
            K::TuiUiAlignLeft
            | K::TuiUiAlignRight
            | K::TuiUiCenter
            | K::TuiUiBold
            | K::TuiUiUnderline
            | K::TuiUiDim
            | K::TuiUiReverse => tui_attr(var(0)),
            K::TuiUiColor | K::TuiUiBg => fun(term_color(), tui_attr(var(0))),
            // ── Ipe.Tea.Cli.Ui line-oriented view + attribute builders ──
            K::CliUiNone => lines_t(var(0)),
            K::CliUiText => fun(string(), lines_t(var(0))),
            // `Cli.Ui.line : List (Attribute msg) -> String -> Lines msg`
            K::CliUiLine => fun(
                list(cli_attr(var(0))),
                fun(string(), lines_t(var(0))),
            ),
            // `Cli.Ui.lines : List (Lines msg) -> Lines msg`
            K::CliUiLines => fun(list(lines_t(var(0))), lines_t(var(0))),
            K::CliUiBold | K::CliUiUnderline | K::CliUiDim | K::CliUiReverse => {
                cli_attr(var(0))
            }
            K::CliUiColor | K::CliUiBg => fun(term_color(), cli_attr(var(0))),
            // ── Ipe.Tea.Terminal.Color palette constructors ──
            K::TermColorBlack
            | K::TermColorRed
            | K::TermColorGreen
            | K::TermColorYellow
            | K::TermColorBlue
            | K::TermColorMagenta
            | K::TermColorCyan
            | K::TermColorWhite
            | K::TermColorBrightBlack
            | K::TermColorBrightRed
            | K::TermColorBrightGreen
            | K::TermColorBrightYellow
            | K::TermColorBrightBlue
            | K::TermColorBrightMagenta
            | K::TermColorBrightCyan
            | K::TermColorBrightWhite
            | K::TermColorDefault => term_color(),
            K::TermColorRgb => fun(int(), fun(int(), fun(int(), term_color()))),
            K::TermColorRgba => fun(
                int(),
                fun(int(), fun(int(), fun(float(), term_color()))),
            ),
            // `widget : CustomElement down up -> down -> (up -> msg) -> Element msg`
            // (msg = var(0), down = var(1), up = var(2)).
            K::UiWidget => fun(
                custom_element(var(1), var(2)),
                fun(
                    var(1),
                    fun(fun(var(2), var(0)), elem_t(var(0))),
                ),
            ),

            // Ipe.Ui / Font attribute builders — nullary (arity 0).
            K::UiCenterX
            | K::UiCenterY
            | K::UiAlignLeft
            | K::UiAlignRight
            | K::UiAlignTop
            | K::UiAlignBottom
            | K::UiPointer
            | K::UiClip
            | K::UiClipX
            | K::UiClipY
            | K::UiScrollbars
            | K::UiScrollbarX
            | K::UiScrollbarY
            | K::FontBold
            | K::FontItalic
            // Tier 1 — nullary Attr
            | K::UiSquare
            | K::UiWidescreen
            | K::UiCinemascope
            | K::BorderSolid
            | K::BorderDashed
            | K::BorderDotted
            | K::FontSemiBold
            | K::FontRegular
            | K::FontLight
            | K::FontExtraBold
            | K::FontBlack
            | K::FontUnderline
            | K::FontNoDecoration
            | K::FontLineThrough
            | K::FontAlignLeft
            | K::FontAlignRight
            | K::FontAlignCenter
            | K::FontCenter
            | K::FontJustify => attr(var(0)),

            // Attribute builders — single Int arg.
            K::UiSpacing
            | K::UiPadding
            | K::UiGridColumns
            | K::BorderWidth
            | K::BorderRounded
            | K::FontSize
            // Tier 1 — Int → Attr
            | K::FontWeight
            | K::FontHoverSize
            | K::BorderHoverWidth
            | K::BorderHoverRounded => fun(int(), attr(var(0))),

            // Attribute builders — single Float arg.
            K::FontLetterSpacing | K::FontWordSpacing | K::UiAspectRatio => {
                fun(float(), attr(var(0)))
            }

            // Attribute builders — Length arg.
            K::UiWidth | K::UiHeight => fun(length(), attr(var(0))),

            // Attribute builders — Color arg.
            K::BackgroundColor
            | K::BorderColor
            | K::FontColor
            // Tier 1 — Color pseudo-class attrs
            | K::BackgroundHoverColor
            | K::BackgroundFocusColor
            | K::BackgroundActiveColor
            | K::BackgroundDisabledColor
            | K::BorderHoverColor
            | K::BorderFocusColor
            | K::BorderActiveColor
            | K::FontHoverColor
            | K::FontFocusColor
            | K::FontActiveColor
            | K::FontDisabledColor => fun(color(), attr(var(0))),

            // Attribute builders — String arg.
            K::BackgroundImage => fun(string(), attr(var(0))),
            K::FontFamily => fun(string(), attr(var(0))),

            // ── Background.linearGradient ────────────────────────────────────────
            // linearGradient : Float -> List (Float, Color) -> Attribute msg
            K::BackgroundLinearGradient => fun(
                float(),
                fun(list(Ty::Tuple(vec![float(), color()])), attr(var(0))),
            ),

            // Ipe.Ui — two Int args (arity 2).
            K::UiPaddingXY => fun(int(), fun(int(), attr(var(0)))),

            // ── Ui.paddingEach ──────────────────────────────────────────────────
            // paddingEach : { top : Int, right : Int, bottom : Int, left : Int }
            //             -> Attribute msg  (same record shape/symbols as
            // Border.widthEach — the `*Each` family shares field names).
            K::UiPaddingEach => {
                let rec_arg = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.edge_f_top, int());
                    m.insert(self.builtins.edge_f_right, int());
                    m.insert(self.builtins.edge_f_bottom, int());
                    m.insert(self.builtins.edge_f_left, int());
                    m
                }, RowTail::Closed);
                fun(rec_arg, attr(var(0)))
            }

            // Tier 1 — two-arg attrs.
            K::UiAspectRatioWH => fun(int(), fun(int(), attr(var(0)))),
            K::UiHtmlAttribute => fun(string(), fun(string(), attr(var(0)))),
            K::UiName => fun(string(), attr(var(0))),
            K::UiStyle => fun(string(), fun(string(), attr(var(0)))),
            // `Ui.transition : String -> Bool -> Attribute msg` — the CSS
            // transition shorthand + a respect-`prefers-reduced-motion` flag.
            // Native surface backing `Ipe.Ui.Transition.attribute` /
            // `attributeUnsafe`.
            K::UiTransitionRaw => fun(string(), fun(bool_ty(), attr(var(0)))),
            // `Ui.gridTracks : String -> String -> Attribute msg` — CSS
            // grid-template-columns (first arg) and grid-template-rows (second arg).
            // Native surface backing `Ipe.Ui.Grid.columns`/`rows`/`tracks`.
            K::UiGridTracksRaw => fun(string(), fun(string(), attr(var(0)))),
            // `Ui.animate : String -> String -> String -> Bool -> Attribute msg`
            // — keyframe-animation name, the animation shorthand tail
            // (`<dur>ms <easing> <delay>ms <iter> <fill>`), the `@keyframes`
            // body, and a respect-`prefers-reduced-motion` flag. Native surface
            // backing `Ipe.Ui.Animation.attribute`.
            K::UiAnimateRaw => fun(
                string(),
                fun(string(), fun(string(), fun(bool_ty(), attr(var(0))))),
            ),

            // Ui.breakpoint + Breakpoint constants.
            //
            // Sanctioned divergence: `Breakpoint` is typed as
            // `String` in the Rust port rather than as a distinct opaque type
            // sanctioned divergence §B-Breakpoint.  Users cannot
            // fabricate arbitrary `Breakpoint` values because all constructors
            // (`mobile`, `tablet`, …) are kernels whose schemes return `string()`;
            // the only type-safety gap vs. the the backend is that a plain `String`
            // literal would also unify — an accepted limitation.
            //
            // `Ui.breakpoint : String -> List (Attribute msg) -> Element msg -> Element msg`
            // `Ui.mediaQuery : String -> List (Attribute msg) -> Element msg -> Element msg`
            // (same shape — `breakpoint` delegates to `mediaQuery` at runtime;
            // `mediaQuery` is the raw-query escape hatch.)
            K::UiBreakpoint | K::UiMediaQuery => fun(
                string(),
                fun(list(attr(var(0))), fun(elem_t(var(0)), elem_t(var(0)))),
            ),

            // ── PseudoClass opaque constants + Ui.onPseudo ──────────────────
            // Typed-constant shortcuts — all return the opaque `PseudoClass` type
            // (mirrors `ipe_runtime::ui::element::PseudoClass`'s 5 constructors).
            K::UiHover | K::UiFocus | K::UiFocusVisible | K::UiActive | K::UiDisabled => {
                pseudo_class()
            }
            // `Ui.onPseudo : PseudoClass -> List (Attribute msg) -> Attribute msg`
            K::UiOnPseudo => fun(
                pseudo_class(),
                fun(list(attr(var(0))), attr(var(0))),
            ),

            // Ipe.Html leaf nodes (arity 1).
            K::HtmlTextNode | K::HtmlRawNode => fun(string(), html_t(var(0))),

            // Ipe.Html generic node (arity 3 — tag, attrs, children). Attrs are
            // `Ipe.Html.Attribute` (html_attr) — matches `Vec<html::Attribute>`.
            K::HtmlNode => fun(
                string(),
                fun(
                    list(html_attr(var(0))),
                    fun(list(html_t(var(0))), html_t(var(0))),
                ),
            ),

            // `Html.voidNode : String -> List Attr -> Html msg` — the generic
            // void counterpart of `Html.node` (arbitrary runtime tag, no
            // children arg). Routes through the same `html_node_` runtime sink
            // with an emit-baked empty children vec.
            K::HtmlVoidNode => fun(string(), fun(list(html_attr(var(0))), html_t(var(0)))),

            // `Html.doctype : List Html -> Html msg` — wraps children in the
            // `!doctype-wrapper` pseudo-tag; `html::render_into_ctx` already
            // recognises it and emits the literal `<!DOCTYPE html>` prefix.
            K::HtmlDoctype => fun(list(html_t(var(0))), html_t(var(0))),

            // `Html.titleNode : String -> Html msg` — wraps a raw string
            // directly in `<title>`.
            K::HtmlTitleNode => fun(string(), html_t(var(0))),

            // `Html.toString : Html msg -> String` — alias of `Html.render`.
            K::HtmlToString => fun(html_t(var(0)), string()),

            // Ipe.Html styleNode (arity 2 — attrs, css string; F7). The
            // runtime bakes `strip_style_close` on the css. RELOCATED — matches
            // the legacy `kernel_ty(Html, styleNode)` byte-for-byte (html_attr +
            // html_t). `List (Ipe.Html.Attribute msg) -> String -> Html msg`.
            K::HtmlStyleNode => fun(list(html_attr(var(0))), fun(string(), html_t(var(0)))),

            // `Html.Unsafe.unsafeScript : String -> Html msg` — an inline
            // `<script>` with a verbatim JavaScript body (FIRST_SCHEMED, Ipê-new,
            // no legacy oracle). The runtime kernel neutralises a `</script`
            // breakout at construction.
            K::HtmlScriptNode => fun(string(), html_t(var(0))),

            // ── Ipe.Html.Attributes retained primitives ─────────────────
            // The fixed-key builders are pure Ipê in `Ipe/Html/Attributes.ipe`
            // over these three `Attribute`-value constructors.
            K::HtmlAttribute => fun(string(), fun(string(), html_attr(var(0)))),
            K::HtmlBoolAttribute => fun(string(), fun(bool_ty(), html_attr(var(0)))),
            K::HtmlNoAttr => html_attr(var(0)),

            // ── Ipe.Ui.Keyed ──────────────────────────────────────────────────
            // `Keyed.column / Keyed.row : List (Attribute msg) -> List (String, Element msg) -> Element msg`
            K::KeyedColumn
            | K::KeyedRow => {
                fun(
                    list(attr(var(0))),
                    fun(list(tuple2(string(), elem_t(var(0)))), elem_t(var(0))),
                )
            }

            // ── Ipe.Decimal ───────────────────────────────────────────────────
            // Construction.
            K::DecZero | K::DecOne | K::DecOneHundred => decimal(),
            K::DecFromString => fun(string(), result(error_ty(), decimal())),
            K::DecFromInt    => fun(int(),    decimal()),
            K::DecFromFloat  => fun(float(),  decimal()),
            K::DecFromMinor  => fun(int(), fun(int(), decimal())),
            // Conversion.
            K::DecToString       => fun(decimal(), string()),
            K::DecToStringFixed  => fun(int(), fun(decimal(), string())),
            K::DecToFloat        => fun(decimal(), float()),
            K::DecToInt          => fun(decimal(), int()),
            K::DecToMinor        => fun(int(), fun(decimal(), int())),
            // Arithmetic.
            K::DecAdd | K::DecSub | K::DecMul => {
                fun(decimal(), fun(decimal(), decimal()))
            }
            K::DecDiv | K::DecMod => {
                fun(decimal(), fun(decimal(), result(error_ty(), decimal())))
            }
            K::DecNeg | K::DecAbs | K::DecFloor | K::DecCeil => {
                fun(decimal(), decimal())
            }
            // Rounding.
            K::DecRound | K::DecRoundHalfUp | K::DecTruncate => {
                fun(int(), fun(decimal(), decimal()))
            }
            // Comparison.
            K::DecCompare => fun(decimal(), fun(decimal(), int())),
            K::DecEq
            | K::DecNeq
            | K::DecLt
            | K::DecLte
            | K::DecGt
            | K::DecGte => fun(decimal(), fun(decimal(), bool_ty())),
            K::DecMin | K::DecMax => fun(decimal(), fun(decimal(), decimal())),
            // Predicates.
            K::DecIsZero | K::DecIsPositive | K::DecIsNegative => {
                fun(decimal(), bool_ty())
            }
            // Percent helpers.
            K::DecPercentOf | K::DecAddPercent | K::DecSubPercent => {
                fun(decimal(), fun(decimal(), decimal()))
            }
            // Formatting.
            // `formatWith : String -> String -> Int -> Decimal -> String`
            K::DecFormatWith => {
                fun(string(), fun(string(), fun(int(), fun(decimal(), string()))))
            }

            // ── Ipe.Money ──────────────────────────────────────────────────────
            // Every kernel takes the currency's ISO 4217 code (a `String`); the
            // compiled-source `Ipe.Money` wrappers do the `Currency -> code`
            // conversion before the call. `Error` here is the runtime `IpeError`
            // channel (`error_ty()`), matching the `Result Error _` runtime sigs.
            K::MoneyFormat | K::MoneyFormatWithCode => {
                fun(string(), fun(decimal(), string()))
            }
            K::MoneyAllocate => {
                fun(int(), fun(int(), fun(decimal(), list(decimal()))))
            }
            K::MoneySetRate => fun(
                string(),
                fun(string(), fun(decimal(), result(error_ty(), Ty::Unit))),
            ),
            K::MoneyGetRate => {
                fun(string(), fun(string(), result(error_ty(), decimal())))
            }
            K::MoneyClearRates => fun(Ty::Unit, result(error_ty(), Ty::Unit)),

            // ── Ipe.Ui.Region ──────────────────────────────────────────
            // Nullary region landmark attrs — `Attribute msg`.
            K::RegionMainContent
            | K::RegionNavigation
            | K::RegionFooter
            | K::RegionAside
            | K::RegionAnnounce
            | K::RegionAnnounceUrgently => attr(var(0)),
            // Arity-1 region attrs.
            K::RegionHeading => fun(int(), attr(var(0))),
            K::RegionLabel => fun(string(), attr(var(0))),

            // ── Ui.describe + desc* constructors ──────────────────────────────
            // `Ui.describe : Description -> Attribute msg`
            K::UiDescribe => fun(description(), attr(var(0))),
            // Nullary `Description` constructors — return `Description`.
            // `descNone`/`descParagraph` back the `node`/`taggedNode` sugar in
            // `Ipe/Ui.ipe`; the rest are `Ui.describe` roles.
            K::UiDescNone
            | K::UiDescParagraph
            | K::UiDescMain
            | K::UiDescNavigation
            | K::UiDescContentInfo
            | K::UiDescComplementary
            | K::UiDescLivePolite
            | K::UiDescLiveAssertive => description(),
            // Arity-1 `Description` constructors.
            K::UiDescHeading => fun(int(), description()),
            K::UiDescLabel => fun(string(), description()),

            // ── Ipe.Ui.Input ──────────────────────────────────────────
            //
            // Label constructors: `List (Attribute msg) -> Element msg -> Label msg`
            K::InputLabelAbove | K::InputLabelBelow | K::InputLabelLeft | K::InputLabelRight => {
                fun(list(attr(var(0))), fun(elem_t(var(0)), label_t(var(0))))
            }
            // `Input.labelHidden : String -> Label msg`
            K::InputLabelHidden => fun(string(), label_t(var(0))),
            // `Input.placeholder : List (Attribute msg) -> Element msg -> Placeholder msg`
            K::InputPlaceholder => {
                fun(list(attr(var(0))), fun(elem_t(var(0)), placeholder_t(var(0))))
            }
            // `Input.text / email / username / search / currentPassword / newPassword`:
            //   List (Attribute msg)
            //   -> { onChange : String -> msg
            //      , text : String
            //      , placeholder : Maybe (Placeholder msg)
            //      , label : Label msg
            //      }
            //   -> Element msg
            K::InputText
            | K::InputEmail
            | K::InputUsername
            | K::InputSearch
            | K::InputCurrentPassword
            | K::InputNewPassword => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(string(), var(0)));
                        m.insert(self.builtins.input_f_text, string());
                        m.insert(
                            self.builtins.input_f_placeholder,
                            maybe(placeholder_t(var(0))),
                        );
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }
            // `Input.multiline`:
            //   List (Attribute msg)
            //   -> { onChange : String -> msg
            //      , text : String
            //      , placeholder : Maybe (Placeholder msg)
            //      , label : Label msg
            //      , spellcheck : Bool
            //      }
            //   -> Element msg
            K::InputMultiline => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(string(), var(0)));
                        m.insert(self.builtins.input_f_text, string());
                        m.insert(
                            self.builtins.input_f_placeholder,
                            maybe(placeholder_t(var(0))),
                        );
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m.insert(self.builtins.input_f_spellcheck, bool_ty());
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }
            // `Input.checkbox`:
            //   List (Attribute msg)
            //   -> { onChange : Bool -> msg
            //      , icon : Bool -> Element msg
            //      , checked : Bool
            //      , label : Label msg
            //      }
            //   -> Element msg
            K::InputCheckbox => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(bool_ty(), var(0)));
                        m.insert(self.builtins.input_f_icon, fun(bool_ty(), elem_t(var(0))));
                        m.insert(self.builtins.input_f_checked, bool_ty());
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }

            // `Input.slider`:
            //   List (Attribute msg)
            //   -> { onChange : String -> msg
            //      , value   : String
            //      , min     : String
            //      , max     : String
            //      , step    : String
            //      , label   : Label msg
            //      }
            //   -> Element msg
            //
            // All numeric values are passed as `String` (matching the DOM's
            // `<input type="range">` wire format); the user parses to a numeric
            // type as needed.
            K::InputSlider => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(string(), var(0)));
                        m.insert(self.builtins.input_f_value, string());
                        m.insert(self.builtins.input_f_min, string());
                        m.insert(self.builtins.input_f_max, string());
                        m.insert(self.builtins.input_f_step, string());
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }

            // ── Ipe.Ui.Input radio group ───────────────────────────────
            //
            // `Input.option : String -> Element msg -> RadioOption msg`
            K::InputOption => fun(string(), fun(elem_t(var(0)), radio_option_t(var(0)))),
            //
            // `Input.radio : List (Attr msg) ->
            //   { onChange : String -> msg
            //   , options  : List (RadioOption msg)
            //   , selected : String
            //   , label    : Label msg
            //   } -> Element msg`
            K::InputRadio => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(string(), var(0)));
                        m.insert(
                            self.builtins.input_f_options,
                            list(radio_option_t(var(0))),
                        );
                        m.insert(self.builtins.input_f_selected, string());
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }
            //
            // `Input.radioRow` — identical signature to `radio`.
            K::InputRadioRow => {
                let cfg_rec = Ty::Record(
                    {
                        let mut m = BTreeMap::new();
                        m.insert(self.builtins.input_f_on_change, fun(string(), var(0)));
                        m.insert(
                            self.builtins.input_f_options,
                            list(radio_option_t(var(0))),
                        );
                        m.insert(self.builtins.input_f_selected, string());
                        m.insert(self.builtins.btn_f_label, label_t(var(0)));
                        m
                    },
                    RowTail::Closed,
                );
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }

            // ── Ipe.Ui.Lazy ────────────────────────────────────────────
            // lazy  : (a -> Element msg) -> a -> Element msg
            K::LazyLazy => fun(
                fun(var(0), elem_t(var(1))),
                fun(var(0), elem_t(var(1))),
            ),
            // lazy2 : (a -> b -> Element msg) -> a -> b -> Element msg
            K::LazyLazy2 => fun(
                fun(var(0), fun(var(1), elem_t(var(2)))),
                fun(var(0), fun(var(1), elem_t(var(2)))),
            ),
            // lazy3 : (a -> b -> c -> Element msg) -> a -> b -> c -> Element msg
            K::LazyLazy3 => fun(
                fun(var(0), fun(var(1), fun(var(2), elem_t(var(3))))),
                fun(var(0), fun(var(1), fun(var(2), elem_t(var(3))))),
            ),
            // lazy4 : (a -> b -> c -> d -> Element msg) -> a -> b -> c -> d -> Element msg
            K::LazyLazy4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), elem_t(var(4)))))),
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), elem_t(var(4)))))),
            ),
            // lazy5 : (a -> b -> c -> d -> e -> Element msg) -> a -> b -> c -> d -> e -> Element msg
            K::LazyLazy5 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), fun(var(4), elem_t(var(5))))))),
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), fun(var(4), elem_t(var(5))))))),
            ),

            // ── Json.Decode (17) — mirrors the already-relocated `Db.Decode`
            //    shapes (function-first `map`/`andThen`; `dec(a)` is the opaque
            //    `Decoder a`). Primitives are arity-0 bare decoders. ──
            K::JsonDecString => dec(string()),
            K::JsonDecInt => dec(int()),
            K::JsonDecFloat => dec(float()),
            K::JsonDecBool => dec(bool_ty()),
            K::JsonDecValue => dec(value()),
            K::JsonDecDecodeString => fun(dec(var(0)), fun(string(), result(error_ty(), var(0)))),
            K::JsonDecDecodeValue => fun(dec(var(0)), fun(value(), result(error_ty(), var(0)))),
            K::JsonDecField => fun(string(), fun(dec(var(0)), dec(var(0)))),
            K::JsonDecAt => fun(list(string()), fun(dec(var(0)), dec(var(0)))),
            K::JsonDecIndex => fun(int(), fun(dec(var(0)), dec(var(0)))),
            K::JsonDecList => fun(dec(var(0)), dec(list(var(0)))),
            K::JsonDecNullable => fun(dec(var(0)), dec(maybe(var(0)))),
            K::JsonDecMap => fun(fun(var(0), var(1)), fun(dec(var(0)), dec(var(1)))),
            K::JsonDecAndThen => fun(fun(var(0), dec(var(1))), fun(dec(var(0)), dec(var(1)))),
            K::JsonDecSucceed => fun(var(0), dec(var(0))),
            K::JsonDecFail => fun(string(), dec(var(0))),
            K::JsonDecOneOf => fun(list(dec(var(0))), dec(var(0))),
            K::JsonDecMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dec(var(0)), fun(dec(var(1)), dec(var(2)))),
            ),
            K::JsonDecMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(dec(var(0)), fun(dec(var(1)), fun(dec(var(2)), dec(var(3))))),
            ),
            K::JsonDecMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    dec(var(0)),
                    fun(dec(var(1)), fun(dec(var(2)), fun(dec(var(3)), dec(var(4))))),
                ),
            ),

            // ── Json.Decode.Pipeline (4) — mirrors `Db.Decode.required` /
            //    `optional`; `next_decoder : Decoder (a -> b)`. ──
            K::JsonDecPRequired => fun(
                string(),
                fun(dec(var(0)), fun(dec(fun(var(0), var(1))), dec(var(1)))),
            ),
            K::JsonDecPRequiredAt => fun(
                list(string()),
                fun(dec(var(0)), fun(dec(fun(var(0), var(1))), dec(var(1)))),
            ),
            K::JsonDecPOptional => fun(
                string(),
                fun(
                    dec(var(0)),
                    fun(var(0), fun(dec(fun(var(0), var(1))), dec(var(1)))),
                ),
            ),
            K::JsonDecPCustom => fun(dec(var(0)), fun(dec(fun(var(0), var(1))), dec(var(1)))),

            // ── Ipe.Config (16) — the shared `Decoder a` carrier (`dec(a)`),
            //    over TOML/YAML/JSON. Combinator/primitive schemes are identical
            //    to `Json.Decode`'s (same runtime `decode_*` fns); the format
            //    front-ends put the source `String` FIRST, then the decoder. ──
            K::ConfigString => dec(string()),
            K::ConfigInt => dec(int()),
            K::ConfigFloat => dec(float()),
            K::ConfigBool => dec(bool_ty()),
            K::ConfigNullable => fun(dec(var(0)), dec(maybe(var(0)))),
            K::ConfigField => fun(string(), fun(dec(var(0)), dec(var(0)))),
            K::ConfigAt => fun(list(string()), fun(dec(var(0)), dec(var(0)))),
            K::ConfigList => fun(dec(var(0)), dec(list(var(0)))),
            K::ConfigSucceed => fun(var(0), dec(var(0))),
            K::ConfigFail => fun(string(), dec(var(0))),
            K::ConfigMap => fun(fun(var(0), var(1)), fun(dec(var(0)), dec(var(1)))),
            K::ConfigAndThen => fun(fun(var(0), dec(var(1))), fun(dec(var(0)), dec(var(1)))),
            // `Config.map2..8 : (a -> .. -> r) -> Decoder a -> .. -> Decoder r`.
            K::ConfigMap2 => fun(
                fun(var(0), fun(var(1), var(2))),
                fun(dec(var(0)), fun(dec(var(1)), dec(var(2)))),
            ),
            K::ConfigMap3 => fun(
                fun(var(0), fun(var(1), fun(var(2), var(3)))),
                fun(
                    dec(var(0)),
                    fun(dec(var(1)), fun(dec(var(2)), dec(var(3)))),
                ),
            ),
            K::ConfigMap4 => fun(
                fun(var(0), fun(var(1), fun(var(2), fun(var(3), var(4))))),
                fun(
                    dec(var(0)),
                    fun(
                        dec(var(1)),
                        fun(dec(var(2)), fun(dec(var(3)), dec(var(4)))),
                    ),
                ),
            ),
            K::ConfigMap5 => fun(
                fun(
                    var(0),
                    fun(var(1), fun(var(2), fun(var(3), fun(var(4), var(5))))),
                ),
                fun(
                    dec(var(0)),
                    fun(
                        dec(var(1)),
                        fun(
                            dec(var(2)),
                            fun(dec(var(3)), fun(dec(var(4)), dec(var(5)))),
                        ),
                    ),
                ),
            ),
            K::ConfigMap6 => fun(
                fun(
                    var(0),
                    fun(
                        var(1),
                        fun(var(2), fun(var(3), fun(var(4), fun(var(5), var(6))))),
                    ),
                ),
                fun(
                    dec(var(0)),
                    fun(
                        dec(var(1)),
                        fun(
                            dec(var(2)),
                            fun(
                                dec(var(3)),
                                fun(dec(var(4)), fun(dec(var(5)), dec(var(6)))),
                            ),
                        ),
                    ),
                ),
            ),
            K::ConfigMap7 => fun(
                fun(
                    var(0),
                    fun(
                        var(1),
                        fun(
                            var(2),
                            fun(var(3), fun(var(4), fun(var(5), fun(var(6), var(7))))),
                        ),
                    ),
                ),
                fun(
                    dec(var(0)),
                    fun(
                        dec(var(1)),
                        fun(
                            dec(var(2)),
                            fun(
                                dec(var(3)),
                                fun(
                                    dec(var(4)),
                                    fun(dec(var(5)), fun(dec(var(6)), dec(var(7)))),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
            K::ConfigMap8 => fun(
                fun(
                    var(0),
                    fun(
                        var(1),
                        fun(
                            var(2),
                            fun(
                                var(3),
                                fun(var(4), fun(var(5), fun(var(6), fun(var(7), var(8))))),
                            ),
                        ),
                    ),
                ),
                fun(
                    dec(var(0)),
                    fun(
                        dec(var(1)),
                        fun(
                            dec(var(2)),
                            fun(
                                dec(var(3)),
                                fun(
                                    dec(var(4)),
                                    fun(
                                        dec(var(5)),
                                        fun(dec(var(6)), fun(dec(var(7)), dec(var(8)))),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
            // `Config.oneOf : List (Decoder a) -> Decoder a`.
            K::ConfigOneOf => fun(list(dec(var(0))), dec(var(0))),
            // `Config.index : Int -> Decoder a -> Decoder a`.
            K::ConfigIndex => fun(int(), fun(dec(var(0)), dec(var(0)))),
            // `Config.keyValuePairs : Decoder a -> Decoder (List (String, a))`.
            K::ConfigKeyValuePairs => {
                fun(dec(var(0)), dec(list(Ty::Tuple(vec![string(), var(0)]))))
            }
            // `Config.maybe : Decoder a -> Decoder (Maybe a)`.
            K::ConfigMaybe => fun(dec(var(0)), dec(maybe(var(0)))),
            // `Config.dict : Decoder a -> Decoder (Dict String a)`.
            K::ConfigDict => fun(dec(var(0)), dec(dict(string(), var(0)))),
            K::ConfigDecodeToml => {
                fun(string(), fun(dec(var(0)), result(error_ty(), var(0))))
            }
            K::ConfigDecodeYaml => {
                fun(string(), fun(dec(var(0)), result(error_ty(), var(0))))
            }
            K::ConfigDecodeJson => {
                fun(string(), fun(dec(var(0)), result(error_ty(), var(0))))
            }
            K::ConfigLoadFromFile => fun(path(), fun(dec(var(0)), task(var(0)))),

            // ── Result (internal) — `okDefault : a -> Result e a`, the Ok-wrap
            //    used during lowering (runtime `ok_res(a) -> Result e a`). ──
            K::ResultOkDefault => fun(var(0), result(var(1), var(0))),

            // ── Ipe.Ui Length builders (result type `Length`) — runtime
            //    `ui_px_(i64) -> Length`, `ui_fill_() -> Length`, etc. `Length`
            //    lowers to `IrType::UiPlain(UiPlain::Length)`. Arrow-count ==
            //    `decl().arity` for every arm. ──
            K::UiPx | K::UiFillPortion | K::UiVh | K::UiVw => fun(int(), length()),
            K::UiFill | K::UiContent | K::UiShrink => length(),
            K::UiMinimum | K::UiMaximum => fun(int(), fun(length(), length())),

            // ── Ipe.Ui Color builders (result type `Color`) — runtime
            //    `ui_rgb_(i64,i64,i64) -> Color`, `ui_rgba_(i64,i64,i64,f64) ->
            //    Color`, `ui_white_() -> Color`, etc. `Color` lowers to
            //    `IrType::UiPlain(UiPlain::Color)`. ──
            K::UiRgb => fun(int(), fun(int(), fun(int(), color()))),
            K::UiRgba => fun(int(), fun(int(), fun(int(), fun(float(), color())))),
            K::UiWhite | K::UiBlack | K::UiTransparent => color(),
            // colorCss : Color -> String
            K::UiColorCss => fun(color(), string()),

            // ── Ipe.Json.Encode (8) — the `JsonEnc.*` encoders. `Value =
            //    any` maps to `IrType::Json` (`JsonVal`) via the `"Value"` arm in
            //    `ipe_lower::ir_type_from_ty`. Runtime: `json_enc_string(String)
            //    -> JsonVal`, `json_enc_null() -> JsonVal` (arity 0),
            //    `json_enc_list(impl Fn(A) -> JsonVal, Vec<A>) -> JsonVal`,
            //    `json_enc_object(Vec<(String, JsonVal)>) -> JsonVal`,
            //    `json_enc_encode(i64, JsonVal) -> String`. Scheming these closes
            //    the former `Ty::Var(u32::MAX)` exit-0 hole (the lowerer's
            //    hardcoded `kernel_native_ir_type` fallback stays as a safety
            //    net for bare-value references). ──
            K::JsonEncString => fun(string(), value()),
            K::JsonEncInt => fun(int(), value()),
            K::JsonEncFloat => fun(float(), value()),
            K::JsonEncBool => fun(bool_ty(), value()),
            K::JsonEncNull => value(),
            K::JsonEncList => fun(fun(var(0), value()), fun(list(var(0)), value())),
            K::JsonEncObject => fun(list(tuple2(string(), value())), value()),
            K::JsonEncEncode => fun(int(), fun(value(), string())),

            // ── Ipe.Error (15 — real Error/ErrorKind/ErrorDetails ADT) ──
            //    `Error` is `Error ErrorKind ErrorInfo` (`Error`'s own ctor scheme
            //    is registered in `ctor_schemes()`), backed at runtime by the real
            //    `ipe_runtime::error::IpeError` enum (`IrType::Error`), not
            //    string-backed. The eight message constructors are `String ->
            //    Error` (each classifies its `ErrorKind` at construction);
            //    `timeout`/`notFound`/`permissionDenied` are nullary `Error`;
            //    `toString : Error -> String` routes through the shared
            //    Stringify-bounded mechanism (see the `BasicsToString |
            //    ErrorToString` special case above — this scheme arm is a
            //    shadowed fallback, never actually reached); `withMessage :
            //    String -> Error -> Error`; `isRetryable : Error -> Bool`
            //    classifies on kind alone; `withDetails : ErrorDetails -> Error ->
            //    Error` attaches the `ErrorDetails` union
            //    to `ErrorInfo.details : Maybe ErrorDetails` (`ErrorDetails`'s own
            //    5-variant ctor scheme is registered in `ctor_schemes()`).
            K::ErrorUnexpected
            | K::ErrorInvalidInput
            | K::ErrorIo
            | K::ErrorNetwork
            | K::ErrorFfi
            | K::ErrorDecode
            | K::ErrorConflict
            | K::ErrorUnavailable => fun(string(), error_ty()),
            K::ErrorTimeout | K::ErrorNotFound | K::ErrorPermissionDenied => error_ty(),
            K::ErrorToString => fun(var(0), string()),
            K::ErrorWithMessage => fun(string(), fun(error_ty(), error_ty())),
            K::ErrorIsRetryable => fun(error_ty(), bool_ty()),
            K::ErrorWithDetails => fun(errordetails_ty(), fun(error_ty(), error_ty())),
            //    Inspectors: `kind : Error -> ErrorKind` and `message : Error ->
            //    String` destructure a live error; `kindName : ErrorKind ->
            //    String` renders a kind's stable label (the same label
            //    `Error.toString` prefixes with).
            K::ErrorKind => fun(error_ty(), errorkind_ty()),
            K::ErrorMessage => fun(error_ty(), string()),
            K::ErrorKindName => fun(errorkind_ty(), string()),

            // ── Ipe.CssSafety (4 — Ipe.Css leaf security kernels) ──
            //    The three parsers are `String -> Maybe String` (`None` => the
            //    Ipê side drops the declaration/rule via `CssDropped` /
            //    `CssRuleDropped`); `stripStyleClose` is the `String -> String`
            //    breakout floor. Runtime `safe_value` / `safe_prop_name` /
            //    `safe_selector` return `IpeMaybe<String>` (mirrors `uuid_parse`);
            //    `strip_style_close_kernel` returns `String`.
            K::CssSafetySafeValue
            | K::CssSafetySafePropName
            | K::CssSafetySafeSelector
            | K::CssSafetySanitizeRawBody => fun(string(), maybe(string())),

            // ── Ipe.Uuid (3) — ENTROPY IS AN EFFECT ──
            //    `v4`/`v7` draw fresh entropy per call, so they are typed on the
            //    effect tier `() -> Task Error String` (runtime `uuid_v4::<E>(_:
            //    ())` / `uuid_v7::<E>(_: ())` return `IpeTask<E, String>`),
            //    called `Uuid.v4 ()` exactly like `Time.now ()`. This makes
            //    "entropy typed as a memoizable pure `String`" unrepresentable —
            //    a pure `String` is CSE/memoization-eligible, so two references
            //    could collapse to one shared value (the soundness lie a shared-ref implementation
            //    backend still carries via bare `Uuid.v4 : String`). `parse`
            //    stays PURE (`String -> Maybe String`): it inspects an existing
            //    string with no entropy — a genuine parser, NOT the arity-0
            //    codegen artifact.
            K::UuidV4 | K::UuidV7 => fun(Ty::Unit, task(string())),
            K::UuidParse => fun(string(), maybe(string())),

            // EXCLUDED — the ONLY kernels without a scheme. This is an
            // EXPLICIT wildcard-free arm, so F1 is structurally
            // unrepresentable here: a future `StdlibKernel` variant fails to
            // compile in `ipe_types` until it is either schemed above or added to
            // one of the two exclusion buckets below).
            //
            //  * `Web.appRouted` — REACHABLE_BUT_UNLOWERED: has a runtime fn +
            //    qualifier, but its lowering is `Feature::RoutedWebApp`
            //    unsupported and its type is a closed record, not a curried `Ty`.
            //    A caller fails closed at type-check until routed lowering lands.
            //
            // Gate-checked (`known_unbacked_never_schemed`,
            // `stdlib_scheme_total_over_reachable`, the REACHABLE_BUT_UNLOWERED
            // disjointness guard). Do NOT add a bare `_` back — it reopens F1.
            //
            //  * `Sub.subscribeTopic` / `Cmd.publish` / `Cmd.publishNoEcho` /
            //    `PubSub.publish` / `PubSub.publishNoEcho` are wired and have
            //    their schemes above; not in this arm.

            // ── Shape-carrying monomorphic families ──
            // Every kernel below carries a structural `TyShape`
            // (`StdlibKernel::scheme_shape`), so `resolve_scheme` types it by
            // interpreting that shape and never consults this table — its scheme
            // lives once, on the descriptor, not as an arm here. Each resolves to
            // `Some` through `resolve_scheme`; the byte-identity of every
            // interpreted shape is pinned by `interpreted_shape_matches_legacy`.
            // The explicit `return None` keeps this match wildcard-free (a new
            // variant must still be classified) while the shape stays the SSOT.
            K::BasicsNot | K::BasicsSqrt | K::BitwiseAnd | K::BitwiseComplement |
            K::BitwiseOr | K::BitwiseShiftLeftBy | K::BitwiseShiftRightBy | K::BitwiseShiftRightZfBy |
            K::BitwiseXor | K::BytesAppend | K::BytesEmpty | K::BytesFromString |
            K::BytesIsEmpty | K::BytesLength | K::BytesSlice | K::BytesToBase64 |
            K::BytesToHex | K::CharFromCode | K::CharIsAlpha | K::CharIsAlphaNum |
            K::CharIsDigit | K::CharIsHexDigit | K::CharIsLower | K::CharIsOctDigit |
            K::CharIsUpper | K::CharToCode | K::CharToLower | K::CharToUpper |
            K::CryptoConstantTimeEqual |
            K::CryptoMd5 | K::CryptoRsaSha256Verify | K::CryptoSha1 |
            K::CryptoSha256 | K::CryptoSha512 | K::CssSafetyStripStyleClose | K::EncodingBase64Encode |
            K::EncodingHexEncode | K::EncodingUrlEncode | K::FontMonospace | K::FontSansSerif |
            K::FontSerif | K::HtmlEscapeAttr | K::HtmlEscapeText | K::MathAbs |
            K::MathAcos | K::MathAcosh | K::MathAsin | K::MathAsinh |
            K::MathAtan | K::MathAtan2 | K::MathAtanh | K::MathCbrt |
            K::MathCeil | K::MathCos | K::MathCosh | K::MathE |
            K::MathExp | K::MathExp2 | K::MathFloor | K::MathHypot |
            K::MathInf | K::MathIsNaN | K::MathLog | K::MathLog10 |
            K::MathLog2 | K::MathMod | K::MathNan | K::MathPhi |
            K::MathPi | K::MathPow | K::MathRemainder | K::MathRound |
            K::MathSin | K::MathSinh | K::MathSqrt | K::MathSqrt2 |
            K::MathTan | K::MathTanh | K::MathTrunc | K::MoneyCurrencyName |
            K::MoneyHasRate | K::MoneyIsKnownCurrency | K::MoneyMinorUnits | K::MoneySymbol |
            K::RateLimitAllow | K::StringAll | K::StringAny | K::StringAppend |
            K::StringCasefold | K::StringCons | K::StringContains | K::StringContainsIn |
            K::StringDropLeft | K::StringDropRight | K::StringEndsWith | K::StringEndsWithIn |
            K::StringEqualFold | K::StringFilter | K::StringFromChar | K::StringFromFloat |
            K::StringFromInt | K::StringIsEmail | K::StringIsEmpty | K::StringIsUrl |
            K::StringLeft | K::StringLength | K::StringMap | K::StringPad |
            K::StringPadLeft | K::StringPadRight | K::StringRepeat | K::StringReplace |
            K::StringReverse | K::StringRight | K::StringSlice | K::StringStartsWith |
            K::StringStartsWithIn | K::StringToLower | K::StringToUpper | K::StringTrim |
            K::StringTrimEnd | K::StringTrimStart | K::SystemGetenvOr | K::TimeDaysInMonth |
            K::TimeIsLeapYear | K::TimeTimeString | K::TimeFormat | K::TimeFormatHTTP |
            K::TimeFormatISO8601 | K::TimeFormatRFC3339 | K::TimeAddMillis | K::TimeDiffMillis |
            K::UiDarkMode | K::UiDesktop |
            K::UiLightMode | K::UiMobile | K::UiReducedMotion | K::UiTablet => return None,

            K::WebAppRouted => return None,

            // ── Ipe.Auth (9 kernels) ──────────────────────────────────────
            // hashPassword : String -> Result Error String
            K::AuthHashPassword => fun(string(), result(error_ty(), string())),
            // hashPasswordCost : String -> Int -> Result Error String
            K::AuthHashPasswordCost => fun(string(), fun(int(), result(error_ty(), string()))),
            // verifyPassword : String -> String -> Result Error Bool
            K::AuthVerifyPassword => fun(string(), fun(string(), result(error_ty(), bool_ty()))),
            // passwordStrength : String -> Result Error String
            K::AuthPasswordStrength => fun(string(), result(error_ty(), string())),
            // signToken : Secret -> Dict String String -> Int -> Result Error String
            // AUD-06 (seal): a flex `claims` `var(0)` would unify with ANY
            // type, so ipe would accept a record/Int/whatever as claims while
            // the emitted wrapper is pinned to `HashMap<String,String>`
            // (project.rs AUTH_WRAPPERS + runtime/auth.rs), no coercion at
            // lowering → cargo fail on any non-Dict claims (exit-0-then-cargo-
            // fail). Pinned concrete per the concrete-over-generic rule — this
            // was never genuine polymorphism, just an unpinned wildcard.
            // Diverges from the polymorphic `a`; see divergences-from-sky.md.
            //
            // the signing secret is `Secret`, not `String` — "secrets
            // are typed" (PRINCIPLES.md). Re-typed in the same change as `Secret`
            // itself; zero migration cost (no fixture calls this kernel yet).
            // `project.rs`'s `AUTH_WRAPPERS` reveals the `Secret` to the runtime's
            // `String`-typed `auth_sign_token` at the wrapper boundary.
            K::AuthSignToken => fun(
                secret(),
                fun(dict(string(), string()), fun(int(), result(error_ty(), string()))),
            ),
            // verifyToken : Secret -> String -> Result Error (Dict String String)
            // same re-typing as `signToken` above.
            K::AuthVerifyToken => {
                fun(secret(), fun(string(), result(error_ty(), dict(string(), string()))))
            }
            // register : Db -> String -> String -> Task Error Int
            K::AuthRegister => fun(db(), fun(string(), fun(string(), task(int())))),
            // login : Db -> String -> String -> Task Error Int
            K::AuthLogin => fun(db(), fun(string(), fun(string(), task(int())))),
            // setRole : Db -> Int -> String -> Task Error ()
            K::AuthSetRole => fun(db(), fun(int(), fun(string(), task_unit()))),
            // subject : Principal -> String — read the verified subject claim.
            K::AuthSubject => fun(principal_ty(), string()),
            // revokeUser / restoreUser : Principal -> String -> Task Error ()
            K::AuthRevocationRevokeUser | K::AuthRevocationRestoreUser => {
                fun(principal_ty(), fun(string(), task_unit()))
            }
            // revokeSession : Principal -> String -> Int -> Task Error ()
            // The Int is the token's cap (absolute lifetime cap), required so
            // the bounded store can reclaim the entry once it is past expiry.
            K::AuthRevocationRevokeSession => {
                fun(principal_ty(), fun(string(), fun(int(), task_unit())))
            }
            // isRevoked : String -> Task Error Bool
            K::AuthRevocationIsRevoked => fun(string(), task(bool_ty())),

            // ── Ipe.Secret — opaque secret-string wrapper ────
            // `fromString` is the seal (construction boundary); `reveal` is the
            // single greppable un-parse; `redacted` is the explicit "<redacted>"
            // accessor (also what `toString`/interpolation gives automatically —
            // see `ipe_runtime::secret`'s hand-written `IpeStringify` impl). No
            // `ty_is_equatable`/`has_show` denylist needed: `Secret` is a bare
            // nullary `Ty::Con`, so `==`/`toString` stay permitted (safe by
            // construction — see the fix spec §1) while Dict-key/Set-elem/`<`/`>`
            // are already rejected by the existing scalar allowlist in
            // `ipe_types::{concrete_super_ok, emitted_bound_satisfied}`.
            K::SecretFromString => fun(string(), secret()),
            K::SecretReveal => fun(secret(), string()),
            // `Secret.use : Secret -> (String -> a) -> a` — apply the caller's
            // function to the revealed plaintext, return its result. Secret-first
            // (pipe-friendly), matching the `secret_use(s, f)` runtime arg order,
            // so it stays off the `kernel_swaps_first_two` list.
            K::SecretUse => fun(secret(), fun(fun(string(), var(0)), var(0))),
            K::SecretRedacted => fun(secret(), string()),

            // ── Ipe.App runtime-config front door ─────────────────────────
            // `App.fromEnv : String -> Secret` — the sole env-secret seal; a
            // hard-coded credential is a plain `String`, so it cannot reach a
            // `Secret` slot (e.g. `Db.url`).
            K::AppFromEnv => fun(string(), secret()),
            // `App.fromEnvRequired : String -> Secret` — identical signature to
            // `App.fromEnv`; the difference is purely runtime (fail-closed on a
            // missing/empty var), so it type-checks the same way.
            K::AppFromEnvRequired => fun(string(), secret()),
            // `Host.bind : HostMode -> Setting a` — cross-cutting; the shape var
            // is free, so it unifies into any app's settings list. The argument is
            // the closed `HostMode` ADT (a bare `Int` no longer type-checks); each
            // `HostMode` constructor projects to the raw host-bind tag at emit
            // (`0` loopback / `1` all interfaces / `2` env-driven).
            K::HostBind => fun(host_mode(), setting(var(0))),
            // `Log.level : LogLevel -> Setting a` — cross-cutting; the argument is
            // the closed `LogLevel` ADT, projected to its `Int` severity tag.
            K::LogLevelSetting => fun(log_level(), setting(var(0))),
            // `Db.url : Secret -> Setting a` — cross-cutting; the URL is a
            // `Secret`, so it only comes from `App.fromEnv` (a hard-coded
            // `String` credential does not type-check here). This is the
            // security-critical rejection the front door exists to enforce.
            K::DbUrlSetting => fun(secret(), setting(var(0))),
            // `Console.adminToken` / `ingestToken` / `metricsToken :
            // Secret -> Setting a` — cross-cutting `Secret`-typed console/telemetry
            // tokens; same shape as `Db.url`, so the token can only come from
            // `App.fromEnv` / `App.fromEnvRequired` (a hard-coded `String` does not
            // type-check).
            K::ConsoleAdminToken | K::ConsoleIngestToken | K::ConsoleMetricsToken => {
                fun(secret(), setting(var(0)))
            }
            // `Web.csrf : CsrfMode -> Setting Web` — the shape marker is PINNED
            // to `Web`, so this setting rejects a non-web app's settings list; the
            // argument is the closed `CsrfMode` ADT (which has no disabling
            // variant), projected to its `Int` tag.
            K::WebCsrf => fun(csrf_mode(), setting(shape_web())),
            // `Web.sessionTtl : Int -> Setting Web` — seconds; web-pinned.
            K::WebSessionTtl => fun(int(), setting(shape_web())),
            // `Web.authMaxLifetime : Int -> Setting Web` — absolute cap seconds; web-pinned.
            K::WebAuthMaxLifetime => fun(int(), setting(shape_web())),
            // `Web.authSlideWindow : Int -> Setting Web` — rolling re-issue window; web-pinned.
            K::WebAuthSlideWindow => fun(int(), setting(shape_web())),
            // `Web.withRevocation : RevocationMode -> Setting Web` — revocation gate mode; web-pinned.
            K::WebAuthRevocationMode => fun(revocation_mode(), setting(shape_web())),
            // Config-tag ADT constructors — nullary values of their closed types.
            K::HostLoopback | K::HostAllInterfaces | K::HostEnvDriven => host_mode(),
            K::LevelDebug | K::LevelInfo | K::LevelWarn | K::LevelError => log_level(),
            K::WebCsrfStrict | K::WebCsrfInherit => csrf_mode(),
            K::WebRevocationOff | K::WebRevocationStore => revocation_mode(),

            // ── Ipe.Http.Server.Stream (4 kernels) ────────────────────────
            // stream : String -> (StreamWriter -> Task Error ()) -> Task Error Response
            // The callback receives an opaque `StreamWriter` handle; emit/finish
            // consume the same handle directly (no Int unwrap layer needed).
            K::StreamStream => fun(string(), fun(fun(sw(), task_unit()), task(resp()))),
            // emit : String -> StreamWriter -> Task Error ()
            K::StreamEmit => fun(string(), fun(sw(), task_unit())),
            // finish : StreamWriter -> Task Error ()
            K::StreamFinish => fun(sw(), task_unit()),
            // withContentType : String -> StreamWriter -> Task Error ()
            K::StreamWithContentType => fun(string(), fun(sw(), task_unit())),

            // ── Ipe.Http.Stream (4 kernels) ──────────────────────────
            // open : HttpRequest -> Task Error StreamId
            //
            // Returns an opaque `StreamId` handle wrapping the raw i64 stream
            // registry key.  Typed to match upstream
            // `Ipe.Http.Stream.open`'s declared return type.
            K::HttpStreamOpen => fun(http_request(), task(stream_id())),
            // forEachChunk : StreamId -> (String -> Task Error ()) -> Task Error ()
            K::HttpStreamForEachChunk => {
                fun(stream_id(), fun(fun(string(), task_unit()), task_unit()))
            }
            // close : StreamId -> Task Error ()
            K::HttpStreamClose => fun(stream_id(), task_unit()),
            // chunks : StreamId -> (ChunkEvent -> msg) -> Sub msg
            // ChunkEvent is opaque from the runtime; modelled as `var(0)`.
            K::HttpStreamChunks => fun(stream_id(), fun(fun(var(0), var(1)), sub(var(1)))),

            // ── Ipe.Http.Server.WebSocket (12 kernels) ────────────────────
            // defaultCfg : WebSocketServerCfg
            // Arity-0: the return type IS the scheme (no `fun` wrapper).
            K::WsDefaultCfg => wscfg(),
            // withOnConnect : (WebSocketServer -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithOnConnect => fun(fun(wsh(), task_unit()), fun(wscfg(), wscfg())),
            // withOnMessage : (WebSocketServer -> String -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithOnMessage => {
                fun(fun(wsh(), fun(string(), task_unit())), fun(wscfg(), wscfg()))
            }
            // withOnClose : (WebSocketServer -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithOnClose => fun(fun(wsh(), task_unit()), fun(wscfg(), wscfg())),
            // withOnError : (WebSocketServer -> Error -> Task Error ()) -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithOnError => {
                fun(fun(wsh(), fun(error_ty(), task_unit())), fun(wscfg(), wscfg()))
            }
            // withMaxMessageBytes : Int -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithMaxMessageBytes => fun(int(), fun(wscfg(), wscfg())),
            // withOriginPatterns : List String -> WebSocketServerCfg -> WebSocketServerCfg
            K::WsWithOriginPatterns => fun(list(string()), fun(wscfg(), wscfg())),
            // upgrade : Request -> WebSocketServerCfg -> Task Error Response
            K::WsUpgrade => fun(req(), fun(wscfg(), task(resp()))),
            // sendToClient : WebSocketServer -> String -> Task Error ()
            K::WsSendToClient => fun(wsh(), fun(string(), task_unit())),
            // sendBinaryToClient : WebSocketServer -> Bytes -> Task Error ()
            K::WsSendBinaryToClient => fun(wsh(), fun(bytes(), task_unit())),
            // broadcast : List WebSocketServer -> String -> Task Error ()
            K::WsBroadcast => fun(list(wsh()), fun(string(), task_unit())),
            // closeClient : WebSocketServer -> Task Error ()
            K::WsCloseClient => fun(wsh(), task_unit()),

            // ── Ipe.WebSocket — outbound WebSocket client ─────────────
            // The Task-tier six take/return a raw `Int` socket id (the stdlib
            // wraps it in the `WebSocket` ADT). `connectWith` takes the nominal
            // `WebSocketCfg` record (`{ url, headers, timeout, pingInterval }`),
            // which the lowerer folds to the runtime `WsClientCfg` struct.
            //
            // connect : String -> Task Error Int
            K::WebSocketConnect => fun(string(), task(int())),
            // connectWith : WebSocketCfg -> Task Error Int
            K::WebSocketConnectWith => fun(wsclientcfg(), task(int())),
            // send : Int -> String -> Task Error ()
            K::WebSocketSend => fun(int(), fun(string(), task_unit())),
            // sendBinary : Int -> Bytes -> Task Error ()
            // Our fork's `Bytes` is a distinct primitive (`Vec<u8>`), matching the
            // runtime `web_socket_send_binary`'s `Vec<u8>` payload (the server-side
            // `sendBinaryToClient` uses the same `bytes()` scheme). Divergence from
            // the reference's stale `String` alias, recorded in the stdlib source.
            K::WebSocketSendBinary => fun(int(), fun(bytes(), task_unit())),
            // close : Int -> Task Error ()
            K::WebSocketClose => fun(int(), task_unit()),
            // closeWithCode : Int -> String -> Int -> Task Error ()
            K::WebSocketCloseWithCode => {
                fun(int(), fun(string(), fun(int(), task_unit())))
            }
            // subscribeWebSocket : Int -> String -> any -> Sub msg
            // The heterogeneous 3rd arg (a bare `msg` for onOpen, or a
            // `WebSocketMessage -> msg` / `CloseCode -> msg` / `Error -> msg`
            // handler for the other three) is modelled as bare `any` (`var(0)`) —
            // matching the stdlib's `subscribeWebSocketRaw` signature so all four
            // on* wrappers unify. `var(1)` is the Sub's msg. The backend peephole
            // splits on the literal `kind` into the four typed `sub_subscribe_ws_*`
            // runtime fns, each with its own concrete 3rd-arg contract.
            K::SubSubscribeWebSocket => {
                fun(int(), fun(string(), fun(var(0), sub(var(1)))))
            }

            // ── Ipe.Ffi.Js — the raw typed transport across the Ipê↔JS seam ──
            // `send : a -> Cmd msg` — payload `a` = var(0), msg = var(1).
            // `subscribe : Decoder a -> (a -> msg) -> Sub msg` — decoded `a`
            // = var(0), msg = var(1). The seal-legality of the concrete `a` is a
            // structural predicate, not an HM constraint, so it is enforced at
            // lowering (`reject_illegal_js_port_seal`), exactly as the
            // `CustomElement` seal is a canon gate rather than a type-scheme bound.
            K::JsSend => fun(var(0), cmd(var(1))),
            K::JsSubscribe => fun(dec(var(0)), fun(fun(var(0), var(1)), sub(var(1)))),
            // `request : a -> Decoder b -> Task b` — correlated one-shot.
            // Outbound payload `a` = var(0); decoded reply `b` = var(1).
            // The seal-legality of the concrete `a` and the inner type of
            // `Decoder b` are structural predicates enforced at lowering
            // (`reject_illegal_js_port_seal`), not HM constraints.
            K::JsRequest => fun(var(0), fun(dec(var(1)), task(var(1)))),
            // The session-stream primitive. `SessionHandle` is the opaque address;
            // the seal-legality of the concrete open/frame/cmd/terminal types is a
            // structural predicate enforced at lowering, not an HM constraint.
            //   openSession   : openCmd(0) -> Decoder frame(1) -> Task SessionHandle
            //   sessionFrames : SessionHandle -> (frame(0) -> msg(1)) -> Sub msg
            //   sendToSession : SessionHandle -> sessionCmd(0) -> Cmd msg(1)
            //   closeSession  : SessionHandle -> closeCmd(0) -> Decoder terminal(1) -> Task terminal
            K::JsOpenSession => fun(var(0), fun(dec(var(1)), task(session_handle()))),
            K::JsSessionFrames => {
                fun(session_handle(), fun(dec(var(0)), fun(fun(var(0), var(1)), sub(var(1)))))
            }
            K::JsSendToSession => fun(session_handle(), fun(var(0), cmd(var(1)))),
            K::JsCloseSession => {
                fun(session_handle(), fun(var(0), fun(dec(var(1)), task(var(1)))))
            }

            // ── Ipe.Env — build-time-embedded public config ──────────
            // public : String -> Maybe String. Resolves ONLY for a
            // `[wasm] publicEnv`-allowlisted key (`env_public.rs`, a
            // per-project backend-generated file — see `project.rs`'s
            // `render_env_public_rs`); every other key is `Nothing`.
            K::EnvPublic => fun(string(), maybe(string())),

            // ── Ipe.Regex (6 kernels) ────────────────────────────────
            // Concrete, monomorphic schemes (no type vars). `compile` parses a
            // pattern String ONCE into the opaque `Regex` handle, surfacing an
            // invalid pattern as a typed `Err` (`String -> Result Error Regex`).
            // Every operation then takes the compiled `Regex`: `match` returns
            // `Bool`; `find` a `Maybe String`; `findAll`/`split` a `List
            // String`; `replace` is `Regex -> String -> String -> String`.
            // Runtime is total/pure (`ipe_runtime::regex_kernel::*`,
            // re-exported ungated).
            K::RegexCompile => fun(string(), result(error_ty(), regex())),
            K::RegexMatch => fun(regex(), fun(string(), bool_ty())),
            K::RegexFind => fun(regex(), fun(string(), maybe(string()))),
            K::RegexFindAll => fun(regex(), fun(string(), list(string()))),
            K::RegexReplace => fun(regex(), fun(string(), fun(string(), string()))),
            K::RegexSplit => fun(regex(), fun(string(), list(string()))),

            // ── Ipe.Path (6 kernels) ─────────────────────────────────
            // `Path` is opaque and validated. `fromString` (the seal) parses a
            // raw `String` into `Result Error Path` — rejecting NUL / traversal
            // escapes at construction; `toString` unwraps back to `String`. The
            // helpers `base`/`dir`/`ext` take a `Path` and return `String`;
            // `isAbsolute` takes a `Path` and returns `Bool`. Runtime total/pure
            // (`ipe_runtime::path::*`, re-exported ungated).
            K::PathFromString => fun(string(), result(error_ty(), path())),
            K::PathToString => fun(path(), string()),
            K::PathBase => fun(path(), string()),
            K::PathDir => fun(path(), string()),
            K::PathExt => fun(path(), string()),
            K::PathIsAbsolute => fun(path(), bool_ty()),

            // ── Ipe.Trace (3 kernels) ─────────────────────────────────────
            // `span : String -> Task a -> Task a` — the wrapped Task's value flows
            // through untouched; the error channel is the implicit `Error`.
            // `event : String -> Task ()`; `attr : String -> String -> Task ()`.
            K::TraceSpan => fun(string(), fun(task(var(0)), task(var(0)))),
            K::TraceEvent => fun(string(), task_unit()),
            K::TraceAttr => fun(string(), fun(string(), task_unit())),

            // ── Ipe.Compression (4 kernels) ───────────────────────────────
            // `Bytes -> Task Bytes` — the Rust runtime `compression_*` takes and
            // returns `Vec<u8>` (`Bytes` lowers to `Vec<u8>`), a documented
            // divergence from the the backend's `String`-as-bytes shape.
            K::CompressionGzip => fun(bytes(), task(bytes())),
            K::CompressionGunzip => fun(bytes(), task(bytes())),
            K::CompressionZstdCompress => fun(bytes(), task(bytes())),
            K::CompressionZstdDecompress => fun(bytes(), task(bytes())),

            // ── Ipe.Csv (5 kernels) ───────────────────────────────────────
            // `Csv` is the closed record `{ header : List String,
            // rows : List (List String) }` (runtime `ipe_runtime::csv::CsvDoc`).
            K::CsvParse => fun(string(), result(error_ty(), csv_rec())),
            K::CsvParseWithDelimiter => {
                fun(string(), fun(string(), result(error_ty(), csv_rec())))
            }
            K::CsvEncode => fun(csv_rec(), string()),
            K::CsvEncodeWithDelimiter => fun(string(), fun(csv_rec(), string())),
            K::CsvParseStreamFromFile => fun(path(), task(list(list(string())))),

            // ── Ipe.Cache (7 kernels) ─────────────────────────────────────
            // All take the raw `Int` handle. `k`/`v` are the surface key/value
            // type variables (`var(0)`/`var(1)`); the runtime scans keys by
            // `PartialEq`. `newRaw` takes the `CacheCfg` record, `stats` returns
            // the `{ hits, misses, evictions }` record.
            K::CacheNewRaw => fun(cachecfg_rec(), task(int())),
            K::CacheGet => fun(int(), fun(var(0), task(maybe(var(1))))),
            K::CachePut => fun(int(), fun(var(0), fun(var(1), task_unit()))),
            K::CacheRemove => fun(int(), fun(var(0), task_unit())),
            K::CacheClear => fun(int(), task_unit()),
            K::CacheSize => fun(int(), task(int())),
            K::CacheStats => fun(int(), task(cache_stats_rec())),

            // ── Ipe.Email ─────────────────────────────────────────────────────────
            // send : EmailProvider -> EmailMessage -> Task Error String
            K::EmailSend => fun(
                email_provider(),
                fun(email_message_rec(), task(string())),
            ),

            // ── Ipe.Crypto typed-key newtypes ────────────────────────────────────
            // Construction boundaries — parse-don't-validate:
            //   fromString : String -> Key
            //   fromBytes  : String -> Key
            // Extraction boundary:
            //   Mac.toHex  : Mac -> String
            K::CryptoKeyFromString => fun(string(), maybe(crypto_key())),
            K::CryptoKeyFromBytes => fun(string(), maybe(crypto_key())),
            K::CryptoMacToHex => fun(crypto_mac(), string()),
            // Typed HMAC variants — Key replaces the bare String role parameter;
            // Mac replaces the bare String return:
            //   hmacSha256WithKey : Key -> String -> Mac
            //   hmacSha512WithKey : Key -> String -> Mac
            K::CryptoHmacSha256WithKey => fun(crypto_key(), fun(string(), crypto_mac())),
            K::CryptoHmacSha512WithKey => fun(crypto_key(), fun(string(), crypto_mac())),

            // ── Ipe.Email.EmailAddress ────────────────────────────────────────────
            // parse-don't-validate boundary — invalid addresses surface as Nothing:
            //   parse    : String -> Maybe EmailAddress
            //   toString : EmailAddress -> String
            K::EmailAddressParse => fun(string(), maybe(email_address())),
            K::EmailAddressToString => fun(email_address(), string()),
            // ── Ipe.Locale ──────────────────────────────────────────────
            K::LocaleFromTag => fun(string(), maybe(locale())),
            K::LocaleToTag => fun(locale(), string()),
            // `toUpperIn`/`toLowerIn`: `Locale -> String -> String`
            K::StringToUpperIn => fun(locale(), fun(string(), string())),
            K::StringToLowerIn => fun(locale(), fun(string(), string())),

            // ── Ipe.Url ───────────────────────────────────────────────────────────
            // parse-don't-validate boundary — an unparseable / relative URL
            // surfaces as `Err`, never a silent accept:
            //   fromString : String -> Result Error Url
            //   toString   : Url -> String
            //   scheme     : Url -> String
            //   host       : Url -> Maybe String
            //   port       : Url -> Maybe Int
            //   path       : Url -> String
            //   query      : Url -> Maybe String
            //   fragment   : Url -> Maybe String
            //   buildQuery : List (String, String) -> String  (percent-encoding)
            K::UrlFromString => fun(string(), result(error_ty(), url())),
            K::UrlToString => fun(url(), string()),
            K::UrlScheme => fun(url(), string()),
            K::UrlHost => fun(url(), maybe(string())),
            K::UrlPort => fun(url(), maybe(int())),
            K::UrlPath => fun(url(), string()),
            K::UrlQuery => fun(url(), maybe(string())),
            K::UrlFragment => fun(url(), maybe(string())),
            K::UrlBuildQuery => fun(list(tuple2(string(), string())), string()),

            // ── Ui.link ──────────────────────────────────────────────────────────
            // link : List (Attribute msg) -> { url : String, label : Element msg }
            //      -> Element msg
            K::UiLink => {
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    // `url : String`
                    m.insert(self.builtins.http_f_url, string());
                    // `label : Element msg`
                    m.insert(self.builtins.btn_f_label, elem_t(var(0)));
                    m
                }, RowTail::Closed);
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }

            // ── Ui.image ─────────────────────────────────────────────────────────
            // image : List (Attribute msg) -> { src : String, description : String }
            //       -> Element msg
            K::UiImage => {
                let cfg_rec = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.img_f_src, string());
                    m.insert(self.builtins.img_f_description, string());
                    m
                }, RowTail::Closed);
                fun(list(attr(var(0))), fun(cfg_rec, elem_t(var(0))))
            }

            // ── Border.widthEach ─────────────────────────────────────────────────
            // widthEach : { top : Int, right : Int, bottom : Int, left : Int }
            //           -> Attribute msg
            K::BorderWidthEach => {
                let rec_arg = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.edge_f_top, int());
                    m.insert(self.builtins.edge_f_right, int());
                    m.insert(self.builtins.edge_f_bottom, int());
                    m.insert(self.builtins.edge_f_left, int());
                    m
                }, RowTail::Closed);
                fun(rec_arg, attr(var(0)))
            }

            // ── Border.shadow ────────────────────────────────────────────────────
            // shadow : { offsetX : Int, offsetY : Int, blur : Int, spread : Int,
            //            color : Color } -> Attribute msg
            K::BorderShadow => {
                let rec_arg = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.shadow_f_offset_x, int());
                    m.insert(self.builtins.shadow_f_offset_y, int());
                    m.insert(self.builtins.shadow_f_blur, int());
                    m.insert(self.builtins.shadow_f_spread, int());
                    m.insert(self.builtins.shadow_f_color, color());
                    m
                }, RowTail::Closed);
                fun(rec_arg, attr(var(0)))
            }

            // ── Border.glow ──────────────────────────────────────────────────────
            // glow : Int -> Color -> Attribute msg  (convenience box-shadow with
            // 0,0 offset + 0 spread; user supplies blur radius + colour). Curried
            // 2-arg — no record, unlike `Border.shadow`.
            K::BorderGlow => fun(int(), fun(color(), attr(var(0)))),

            // ── Border.innerShadow ────────────────────────────────────────────────
            // innerShadow : { offsetX : Int, offsetY : Int, blur : Int, spread : Int,
            //                 color : Color } -> Attribute msg
            // Same record shape as Border.shadow but INSET; reuses the shadow field
            // symbols.
            K::BorderInnerShadow => {
                let rec_arg = Ty::Record({
                    let mut m = BTreeMap::new();
                    m.insert(self.builtins.shadow_f_offset_x, int());
                    m.insert(self.builtins.shadow_f_offset_y, int());
                    m.insert(self.builtins.shadow_f_blur, int());
                    m.insert(self.builtins.shadow_f_spread, int());
                    m.insert(self.builtins.shadow_f_color, color());
                    m
                }, RowTail::Closed);
                fun(rec_arg, attr(var(0)))
            }

            // ── Ipe.Db.Sql — SqlFragment builder ───────────────
            //
            // `Sql.column : String -> SqlFragment` — validated column/table
            // reference (dot-accepting, so `users.id` is legal).
            K::SqlColumn => fun(string(), sqlfragment()),
            // `Ipe.Db.Unsafe.unsafeFragment : String -> SqlFragment` — the
            // un-validated anti-`Sql.column` (same shape, no `valid_sql_ident`).
            K::SqlUnsafeFragment => fun(string(), sqlfragment()),
            // `Sql.param : SqlValue -> SqlFragment` — binds a single `?`.
            K::SqlParam => fun(sqlvalue(), sqlfragment()),
            // `int` / `string` / `float` / `bool` are Ipê-level type
            // narrowings of `param` (sugar — see the kernel decl doc): same
            // shape, scalar argument instead of the `SqlValue` ADT.
            K::SqlInt => fun(int(), sqlfragment()),
            K::SqlString => fun(string(), sqlfragment()),
            K::SqlFloat => fun(float(), sqlfragment()),
            K::SqlBool => fun(bool_ty(), sqlfragment()),
            // `eq/ne/gt/lt/gte/lte/and/or : SqlFragment -> SqlFragment -> SqlFragment`
            K::SqlEq
            | K::SqlNe
            | K::SqlGt
            | K::SqlLt
            | K::SqlGte
            | K::SqlLte
            | K::SqlAnd
            | K::SqlOr => fun(sqlfragment(), fun(sqlfragment(), sqlfragment())),
            // `not/isNull/isNotNull : SqlFragment -> SqlFragment`
            K::SqlNot | K::SqlIsNull | K::SqlIsNotNull => fun(sqlfragment(), sqlfragment()),
            // `inList : SqlFragment -> List SqlValue -> SqlFragment` — `[]`
            // emits `(1 = 0)` at the runtime combinator, not a type-level case.
            K::SqlInList => fun(sqlfragment(), fun(list(sqlvalue()), sqlfragment())),
            // `like : SqlFragment -> String -> SqlFragment` — the pattern is
            // always a bound param.
            K::SqlLike => fun(sqlfragment(), fun(string(), sqlfragment())),
        })
    }
}
