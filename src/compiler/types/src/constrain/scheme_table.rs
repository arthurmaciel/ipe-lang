use super::{
    BTreeMap, Builder, BuiltinTag, FieldTag, RowTail, RowTailShape, SchemeKey, SchemeSlot, Symbol,
    Ty, TyShape,
};

impl Builder<'_> {
    /// Resolve a [`SchemeKey`] carried on a [`ipe_kernels::KernelDef`] to its
    /// concrete HM type scheme.
    ///
    /// A [`SchemeKey`] names a kernel's scheme without carrying it (the scheme is
    /// built from interned `Symbol`s that exist only after the `Interner` runs,
    /// so it cannot be a `'static` value on the descriptor). This is the single
    /// interpreter that turns the key back into a `Ty`: it reads the kernel's
    /// structural [`ipe_kernels::TyShape`] — the one scheme source — and
    /// interprets it via [`Self::interpret_shape`]. `None` means the kernel has no
    /// scheme (a routed / unlowered bucket), so the caller fails closed.
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
        // Every schemed kernel carries a structural `TyShape`, resolved by
        // interpreting it — the single scheme source. A kernel WITHOUT a shape
        // (`shape == None`) is genuinely unschemed (a routed / unlowered
        // bucket), so it resolves to `None` and the caller fails closed.
        let resolved = key.0.def().shape.map(|shape| self.interpret_shape(shape));
        if let Some(slot) = self.scheme_cache.borrow_mut().get_mut(idx) {
            *slot = SchemeSlot::Resolved(resolved.clone());
        }
        resolved
    }

    /// Interpret a `'static` [`TyShape`] into a concrete [`Ty`], resolving each
    /// [`BuiltinTag`] against the interned-symbol cache.
    ///
    /// The single interpreter a structural kernel scheme routes through — the one
    /// source that turns a kernel's `TyShape` into its concrete HM scheme `Ty`.
    ///
    /// It touches no union-find state even for the polymorphic [`TyShape::Var`]
    /// node: a scheme var interprets to a placeholder `Ty::Var` at its bare
    /// positional index (annotation-symbol space), NOT a fresh union-find var.
    /// Generalization / instantiation with fresh solver vars happens later at the
    /// use site (`instantiate_in`), so this interpreter still takes `&self`.
    /// Because `Ty::Var` is `Eq`, repeating an index reuses one variable
    /// structurally without any shared-cell handling.
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
            // Element order is significant and preserved.
            TyShape::Tuple(elems) => {
                Ty::Tuple(elems.iter().map(|e| self.interpret_shape(e)).collect())
            }
            // The `BTreeMap` re-sorts by the resolved field `Symbol`, so the
            // materialised `Ty::Record`'s key order is independent of the declared
            // slice order.
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
            // A scheme-local variable's raw is its bare positional index.
            TyShape::Var(i) => Ty::Var(u32::from(*i)),
            // `()` materialises the bare `Ty::Unit` leaf.
            TyShape::Unit => Ty::Unit,
        }
    }

    /// Resolve a structural [`BuiltinTag`] to the interned type-constructor
    /// [`Symbol`] the interpreter puts in the `Ty::Con` for that built-in.
    #[allow(clippy::too_many_lines)] // one arm per BuiltinTag variant, deliberately exhaustive
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
            BuiltinTag::UrlRelative => self.builtins.url_relative,
            BuiltinTag::Dsn => self.builtins.dsn,
            BuiltinTag::Connection => self.builtins.connection,
            BuiltinTag::ConnReadOnly => self.builtins.conn_read_only,
            BuiltinTag::ConnReadWrite => self.builtins.conn_read_write,
            BuiltinTag::Setting => self.builtins.setting,
            BuiltinTag::ShapeWeb => self.builtins.shape_web,
            BuiltinTag::ShapeWebView => self.builtins.shape_webview,
            BuiltinTag::ShapeTerminal => self.builtins.shape_terminal,
            BuiltinTag::Program => self.builtins.program,
            BuiltinTag::ProgramShapeWeb => self.builtins.program_shape_web,
            BuiltinTag::ProgramShapeTui => self.builtins.program_shape_tui,
            BuiltinTag::ProgramShapeCli => self.builtins.program_shape_cli,
            BuiltinTag::ProgramShapeWorker => self.builtins.program_shape_worker,
            BuiltinTag::HostMode => self.builtins.host_mode,
            BuiltinTag::LogLevel => self.builtins.log_level,
            BuiltinTag::CsrfMode => self.builtins.csrf_mode,
            BuiltinTag::RevocationMode => self.builtins.revocation_mode,
            BuiltinTag::Locale => self.builtins.locale,
            BuiltinTag::HttpMethod => self.builtins.http_method,
            BuiltinTag::RedirectPolicy => self.builtins.redirect_policy,
            BuiltinTag::Duration => self.builtins.duration,
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
            BuiltinTag::View => self.builtins.view,
            BuiltinTag::UiElement => self.builtins.element,
            BuiltinTag::Cells => self.builtins.cells,
            BuiltinTag::TuiAttr => self.builtins.tui_attr,
            BuiltinTag::CliLines => self.builtins.cli_lines,
            BuiltinTag::CliAttr => self.builtins.cli_attr,
            // The unified `Color` and the legacy `Ui.Color` share the interned
            // `"Color"` name; they collapse into one carrier as the migration removes
            // `UiColor`.
            BuiltinTag::Color | BuiltinTag::UiColor => self.builtins.color,
            BuiltinTag::ColorError => self.builtins.color_error,
            BuiltinTag::TermProfile => self.builtins.term_profile,
            BuiltinTag::AnsiColor => self.builtins.ansi_color,
            BuiltinTag::WcagLevel => self.builtins.wcag_level,
            BuiltinTag::TextSize => self.builtins.text_size,
            BuiltinTag::Deficiency => self.builtins.deficiency,
            BuiltinTag::CustomElement => self.builtins.custom_element,
            BuiltinTag::Html => self.builtins.html_con,
            BuiltinTag::UiLength => self.builtins.length,
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
            BuiltinTag::DbStore => self.builtins.store_con,
            BuiltinTag::DbDraft => self.builtins.draft_con,
            BuiltinTag::DbJoined => self.builtins.joined_con,
            BuiltinTag::DbSelect => self.builtins.select_con,
            BuiltinTag::DbPolicy => self.builtins.policy_con,
            BuiltinTag::DbCond => self.builtins.cond_con,
            BuiltinTag::DbPred => self.builtins.pred_con,
            BuiltinTag::DbSecured => self.builtins.secured_con,
            BuiltinTag::DbOrder => self.builtins.order_con,
            BuiltinTag::Codec => self.builtins.codec_con,
        }
    }

    /// Resolve a structural [`FieldTag`] to the interned field-name [`Symbol`]
    /// the interpreter uses as the `Ty::Record` `BTreeMap` key for that field.
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
    /// [`BuiltinTag`] in the `Ty::Con { module, .. }`.
    ///
    /// Most tags are empty-module (unqualified). The homed exceptions carry a
    /// real module home so a point-free reference to the scheme lowers to the
    /// emitted enum instead of missing the lowerer's home-keyed variant lookup:
    /// [`BuiltinTag::HtmlAttribute`] (the `Html` constructor symbol, so
    /// `ir_type_from_ty`'s disambiguation selects the `Html` attribute variant
    /// distinct from the unqualified [`BuiltinTag::UiAttribute`]),
    /// [`BuiltinTag::EmailProvider`], [`BuiltinTag::Duration`], the
    /// `Ipe.Db.Store` query-algebra ADTs, and [`BuiltinTag::Codec`].
    pub fn builtin_con_module(&self, tag: BuiltinTag) -> Vec<Symbol> {
        match tag {
            BuiltinTag::HtmlAttribute => vec![self.builtins.html_con],
            // The `send` kernel takes `EmailProvider` as its first parameter.
            // Carrying the real `Ipe.Email` home lets a point-free reference to
            // the interpreted scheme lower to the emitted enum; without it the
            // unhomed `Con` misses the lowerer's home-keyed variant lookup and
            // drops into the unknown-builtin internal-compiler-error arm.
            BuiltinTag::EmailProvider => self.builtins.email_home.clone(),
            // `Duration` is a compiled-source ADT (`Ipe.Duration.Duration`), not a
            // folded builtin — its `Http.withTimeout` scheme reference must carry
            // the real `["Ipe", "Duration"]` home so a point-free use lowers to the
            // emitted enum, exactly as `EmailProvider` does.
            BuiltinTag::Duration => self.builtins.duration_home.clone(),
            // The `Ipe.Db.Store` query-algebra ADTs carry the store home so a
            // point-free reference lowers to the emitted enum, exactly as the
            // hand-built `store` / `draft` / … helpers did.
            BuiltinTag::DbStore
            | BuiltinTag::DbDraft
            | BuiltinTag::DbJoined
            | BuiltinTag::DbSelect
            | BuiltinTag::DbPolicy
            | BuiltinTag::DbCond
            | BuiltinTag::DbPred
            | BuiltinTag::DbSecured => self.builtins.db_store_home.clone(),
            BuiltinTag::Codec => self.builtins.codec_home.clone(),
            _ => Vec::new(),
        }
    }
}
