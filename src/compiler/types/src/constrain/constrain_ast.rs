use super::*;

impl<'a> Builder<'a> {
    #[allow(clippy::too_many_lines)] // Handler expansion block (E-12) pushes it over 100
    pub(crate) fn constrain_def(&mut self, def: &canon::Def) -> DResult<()> {
        // Track which source module this def belongs to so every `regions.insert`
        // in the sub-expression walk uses `(home, span)` as the key, preventing
        // cross-module span collisions after `link::link` merges dep modules.
        self.current_home = def.home().to_vec();
        match def {
            canon::Def::Typed {
                name,
                patterns,
                body,
                ty,
                free_vars,
                ..
            } => {
                // Instantiate the WHOLE signature through one shared map so every
                // occurrence of an annotation variable (`a` in `a -> a`) becomes
                // the *same* rigid (skolem) node, and distinct variables become
                // distinct rigids. Checking the body against rigids is what makes
                // the annotation a genuine contract: `f : a -> a; f x = x + 1`
                // (body pins `a` to `Int`) and `f : a -> b; f x = x` (body
                // conflates `a` and `b`) are both mismatches rather than silently
                // accepted. Per-call-site uses instead instantiate the binding's
                // type as fresh *flex* variables (see [`Self::instantiate`]).
                // ── Handler alias expansion (T0004 fix) ───────────────
                // `Handler` is the stdlib alias `Request -> Task Error Response`
                // (Ipe.Http.Server).  A binding annotated as `Handler` with one
                // parameter (e.g. `handleHome : Handler; handleHome req = …`)
                // would fire T0004 because the annotation is a nullary `Con`, not
                // a `Lambda`.  Expand it to the full arrow type here, before the
                // parameter-loop runs, so the loop can peel the arrow normally.
                //
                // The expansion is purely canonical — it mirrors exactly what
                // `canonicalise_type` would produce for an explicit
                // `Request -> Task Error Response` annotation.  `handler_expansion`
                // is kept as an owned `canon::Type` so `cursor` (a reference) can
                // point into it when the annotation is `Handler`.
                let handler_expansion: Option<canon::Type> = {
                    if let canon::Type::Con {
                        name: tname, args, ..
                    } = ty
                    {
                        if *tname == self.builtins.handler
                            && args.is_empty()
                            && !patterns.is_empty()
                        {
                            let task_resp = canon::Type::Con {
                                home: Vec::new(),
                                name: self.builtins.task,
                                args: vec![
                                    canon::Type::Con {
                                        home: Vec::new(),
                                        name: self.builtins.error,
                                        args: Vec::new(),
                                    },
                                    canon::Type::Con {
                                        home: Vec::new(),
                                        name: self.builtins.server_response,
                                        args: Vec::new(),
                                    },
                                ],
                            };
                            Some(canon::Type::Lambda(
                                Box::new(canon::Type::Con {
                                    home: Vec::new(),
                                    name: self.builtins.server_request,
                                    args: Vec::new(),
                                }),
                                Box::new(task_resp),
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                let mut rigid_vars = BTreeMap::new();
                let mut local = BTreeMap::new();
                let mut cursor: &canon::Type = handler_expansion.as_ref().unwrap_or(ty);
                for pat in patterns {
                    let (arg_ty, rest) = match cursor {
                        canon::Type::Lambda(a, b) => (a.as_ref(), b.as_ref()),
                        // The binding writes more parameter patterns than its
                        // annotation has arrows (`f a b = …` with `f : Int`).
                        // Parse-don't-validate: surface a user-facing
                        // IPE-T0004 with the binding span + the written
                        // signature, not a CompilerBug.
                        _ => return Err(self.too_many_parameters(name, ty)),
                    };
                    let arg = self.normalize_annotation_ty(from_canon(arg_ty), name.span)?;
                    let arg_var = self.instantiate_rigid(&arg, &mut rigid_vars)?;
                    self.constrain_pattern(&mut local, pat, arg_var)?;
                    // Record the param pattern's region so the lowerer can read the
                    // solved param type (record-param field-set completion, IPE-T0015
                    // path). Keyed by `(current_home, pat.span)` to prevent collisions
                    // across dep modules (see `Builder::regions` doc comment).
                    self.regions
                        .insert((self.current_home.clone(), pat.span), arg_var);
                    cursor = rest;
                }
                let ret_ty = self.normalize_annotation_ty(from_canon(cursor), name.span)?;
                let ret_var = self.instantiate_rigid(&ret_ty, &mut rigid_vars)?;
                let body_var = self.constrain_expr(&local, body)?;
                // A typed binding's body expects its annotation return type —
                // the strongest completion signal: `f : Color; f = ⟨|⟩` offers
                // `Color`'s constructors first.
                self.record_expected(body.span, ret_var);
                self.eq(body.span, body_var, ret_var);
                // A binding whose RETURN annotation is the bare wildcard `any`
                // severs its body's settled type from every use site (each `any`
                // occurrence instantiates its own fresh flex). Record the body
                // var so [`Self::tie_wildcard_any_uses_to_bodies`] can re-connect
                // it to every use, undoing the severance at its root (a
                // `view = <this binding>` with an `Html` body then reaches the
                // shape's `Element` requirement as an ordinary mismatch). The
                // guard mirrors the registration pass exactly
                // ([`Self::annotation_returns_wildcard_any`]): a point-free def
                // (`alias : Model -> any; alias = view`, zero written patterns)
                // leaves `ret_ty` as the whole `Model -> any` arrow, which the
                // tie peels along with the use — so both def forms are recorded.
                if self.annotation_returns_wildcard_any(&ret_ty) {
                    self.wildcard_any_return_bodies
                        .insert((self.current_home.clone(), name.value), body_var);
                }
                // Record the skolem each annotation variable instantiated to, so
                // its body-imposed super-type obligations can be read back for
                // generalisation. Keyed by the variable's symbol (the lowerer's
                // `free_vars` are these same symbols).
                let mut var_rigids = BTreeMap::new();
                for fv in free_vars {
                    if let Some(rigid) = rigid_vars.get(&fv.as_raw()) {
                        var_rigids.insert(*fv, *rigid);
                    }
                }
                self.typed_rigids
                    .push(((self.current_home.clone(), name.value), var_rigids));
                Ok(())
            }
            canon::Def::Untyped {
                name,
                patterns,
                body,
                ..
            } => {
                let mut local = BTreeMap::new();
                let mut param_vars = Vec::with_capacity(patterns.len());
                for pat in patterns {
                    let v = self.flex()?;
                    self.constrain_pattern(&mut local, pat, v)?;
                    self.regions
                        .insert((self.current_home.clone(), pat.span), v);
                    param_vars.push(v);
                }
                let body_var = self.constrain_expr(&local, body)?;
                // Reconstruct the binding's full type as the right-nested arrow
                // `p0 -> p1 -> … -> body`, so `env[f]` for `f a b = a` is
                // `a -> b -> a`, not just the body's type. A binding with no
                // parameters is just its body's type.
                let mut arrow = body_var;
                for pv in param_vars.into_iter().rev() {
                    arrow = self.structure(FlatType::Fun(pv, arrow))?;
                }
                // Tie the reconstructed type to the shared variable minted in the
                // registration pass, which every reference resolves to.
                // Use the same (home, name) key that the registration pass used.
                let shared_key = (def.home().to_vec(), name.value);
                let Some(shared) = self.untyped.get(&shared_key).copied() else {
                    return Err(Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: format!(
                            "untyped binding `{}` was not registered",
                            self.interner.resolve(name.value).unwrap_or("<unknown>")
                        ),
                    });
                };
                self.eq(name.span, arrow, shared);
                Ok(())
            }
        }
    }

    /// Build the IPE-T0004 diagnostic for a binding with more parameter
    /// patterns than its annotation has arrows. Resolving the name / rendering
    /// the signature can itself only fail on a forged symbol, in which case
    /// that internal bug is surfaced instead.
    pub(crate) fn too_many_parameters(
        &self,
        name: &ipe_diagnostics::Located<Symbol>,
        ty: &canon::Type,
    ) -> Diagnostic {
        let binding = match self.interner.resolve(name.value) {
            Some(s) => Box::from(s),
            None => {
                return Diagnostic::CompilerBug {
                    where_: "intern.resolve",
                    detail: format!("no backing string for symbol {}", name.value.as_raw()),
                };
            }
        };
        match canon_type_to_doc(ty, self.interner) {
            Ok(signature) => Diagnostic::Type {
                span: name.span,
                msg: TypeError::TooManyParameters {
                    binding,
                    signature: Box::new(signature),
                },
            },
            Err(bug) => bug,
        }
    }

    /// Whether `ty` is the bare wildcard `any` annotation type — a `Ty::Var`
    /// whose interned symbol resolves to `"any"`. Mirrors the `Ty::Var` "any"
    /// arm in [`Self::instantiate_in`]: `any` is Ipê's wildcard type-variable
    /// name, distinct from a genuine named parameter (`a`, `msg`).
    pub(crate) fn is_wildcard_any_ty(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Var(id) if self
            .interner
            .resolve(Symbol::from_raw(*id))
            .is_some_and(|name| name == "any"))
    }

    /// Whether an annotation type's final RETURN (after peeling every leading
    /// `_ -> _` arrow) is the bare wildcard `any`. Such a binding's body is
    /// severed from its uses by the wildcard and must be re-tied — see
    /// [`Self::tie_wildcard_any_uses_to_bodies`].
    pub(crate) fn annotation_returns_wildcard_any(&self, ty: &Ty) -> bool {
        let mut cur = ty;
        while let Ty::Fun(_, ret) = cur {
            cur = ret;
        }
        self.is_wildcard_any_ty(cur)
    }

    /// Reduce a 2-arg `Task Error a` annotation type to the internal unary
    /// `Task a`, validating that the error channel is the `Error` type, and
    /// recursively normalise nested occurrences in any composite type.
    ///
    /// Ipê mandates `Task Error a` as the canonical user-facing form, but the
    /// type-checker's internal model is unary `Task a` — the error channel is
    /// always `Error` and therefore implicit in the IR.  This bridge is applied
    /// to every result of [`from_canon`] so user annotations unify with the
    /// kernel-built unary forms.
    ///
    /// # Errors
    ///
    /// Returns `IPE-T0001` when the error channel is not `Error` (e.g.
    /// `Task String a` or `Task Int a`).  Returns `IPE-T0016`
    /// ([`TypeError::TaskArity`]) when a `Task` annotation has a number of type
    /// arguments other than 1 or 2 — reachable from source (a bare `Task`, or
    /// `Task Error Int Bool`), because canonicalisation validates arity only for
    /// type *aliases*, never for a non-alias constructor application like `Task`.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn normalize_annotation_ty(&self, ty: Ty, span: Span) -> DResult<Ty> {
        match ty {
            Ty::Con { module, name, args } => {
                if name == self.builtins.task {
                    match args.len() {
                        // 1-arg: already the internal unary form; recurse inside.
                        1 => {
                            let inner =
                                args.into_iter()
                                    .next()
                                    .ok_or_else(|| Diagnostic::CompilerBug {
                                        where_: STAGE,
                                        detail: "Task 1-arg: iterator exhausted (internal)".into(),
                                    })?;
                            let inner = self.normalize_annotation_ty(inner, span)?;
                            Ok(Ty::Con {
                                module,
                                name,
                                args: vec![inner],
                            })
                        }
                        // 2-arg: `Task Error a` — validate error channel, reduce.
                        2 => {
                            let mut it = args.into_iter();
                            let e_ty = it.next().ok_or_else(|| Diagnostic::CompilerBug {
                                where_: STAGE,
                                detail: "Task 2-arg: first arg missing (internal)".into(),
                            })?;
                            let a_ty = it.next().ok_or_else(|| Diagnostic::CompilerBug {
                                where_: STAGE,
                                detail: "Task 2-arg: second arg missing (internal)".into(),
                            })?;
                            if !self.is_error_ty(&e_ty) {
                                // Render both sides for a clear IPE-T0001 diagnostic.
                                let mut namer = VarNamer::new();
                                let expected = ty_to_doc(
                                    &Ty::Con {
                                        module: Vec::new(),
                                        name: self.builtins.error,
                                        args: Vec::new(),
                                    },
                                    self.interner,
                                    &mut namer,
                                )?;
                                let found = ty_to_doc(&e_ty, self.interner, &mut namer)?;
                                return Err(Diagnostic::Type {
                                    span,
                                    msg: TypeError::TypeMismatch {
                                        expected: Box::new(expected),
                                        found: Box::new(found),
                                        definition: None,
                                        path: Box::new([]),
                                    },
                                });
                            }
                            let inner = self.normalize_annotation_ty(a_ty, span)?;
                            Ok(Ty::Con {
                                module,
                                name,
                                args: vec![inner],
                            })
                        }
                        // A `Task` applied to any other arity (bare `Task`, or
                        // `Task Error Int Bool`) is ill-formed. It reaches here
                        // from source because canonicalisation validates arity
                        // only for type *aliases* (`NameError::AliasArity`), never
                        // for a non-alias type-constructor application like `Task`.
                        // Fail closed with a clean IPE-T0016 diagnostic naming the
                        // found argument count instead of raising a `CompilerBug`.
                        n => Err(Diagnostic::Type {
                            span,
                            msg: TypeError::TaskArity {
                                carrier: "Task",
                                found: n,
                            },
                        }),
                    }
                } else if (name == self.builtins.cmd || name == self.builtins.sub)
                    && args.len() != 1
                {
                    // `Cmd` / `Sub` take exactly one message type. A mis-arity
                    // application (bare `Cmd`, `Cmd Int Bool`) would otherwise
                    // reach the lowerer's `ir_type_from_canon` catch-all and
                    // ICE (IPE-I0001) — the Cmd/Sub sibling of the Task gate.
                    // Fail closed here with the same clean IPE-T0016, symmetric
                    // with the `Task` arm above.
                    let carrier = if name == self.builtins.cmd {
                        "Cmd"
                    } else {
                        "Sub"
                    };
                    Err(Diagnostic::Type {
                        span,
                        msg: TypeError::TaskArity {
                            carrier,
                            found: args.len(),
                        },
                    })
                } else if args.is_empty() && self.interner.resolve(name) == Some("HttpRequest") {
                    // `HttpRequest` is a stdlib type alias for a structural record
                    // (`{ body, headers, method, redirects, timeout, url }`).  The Rust port has no Ipê-source stdlib
                    // files, so the canonicaliser never registers `HttpRequest` as a
                    // type alias — it falls through to an opaque `Con`.  Expand it
                    // here so user annotations like `upstreamRequest : HttpRequest`
                    // unify with the structural record that kernels such as
                    // `HttpStreamOpen` / `HttpGet` / `HttpPost` expect.
                    let mk = |n: Symbol| Ty::Con {
                        module: Vec::new(),
                        name: n,
                        args: Vec::new(),
                    };
                    let string = || mk(self.builtins.string);
                    let int = || mk(self.builtins.int);
                    let http_method_ty = || mk(self.builtins.http_method);
                    let redirect_policy_ty = || mk(self.builtins.redirect_policy);
                    let list = |t: Ty| Ty::Con {
                        module: Vec::new(),
                        name: self.builtins.list,
                        args: vec![t],
                    };
                    let mut req_fields = BTreeMap::new();
                    req_fields.insert(self.builtins.http_f_body, string());
                    req_fields.insert(
                        self.builtins.http_f_headers,
                        list(Ty::Tuple(vec![string(), string()])),
                    );
                    req_fields.insert(self.builtins.http_f_method, http_method_ty());
                    req_fields.insert(self.builtins.http_f_redirects, redirect_policy_ty());
                    req_fields.insert(self.builtins.http_f_timeout, int());
                    req_fields.insert(self.builtins.http_f_url, string());
                    Ok(Ty::Record(req_fields, RowTail::Closed))
                } else if args.is_empty() && self.interner.resolve(name) == Some("HttpResponse") {
                    // `HttpResponse` is a stdlib type alias for `{ body : String,
                    // headers : Dict String String, status : Int }`.  Expand for the
                    // same reason as `HttpRequest` above.
                    let mk = |n: Symbol| Ty::Con {
                        module: Vec::new(),
                        name: n,
                        args: Vec::new(),
                    };
                    let string = || mk(self.builtins.string);
                    let int = || mk(self.builtins.int);
                    let dict = |k: Ty, v: Ty| Ty::Con {
                        module: Vec::new(),
                        name: self.builtins.dict,
                        args: vec![k, v],
                    };
                    let mut resp_fields = BTreeMap::new();
                    resp_fields.insert(self.builtins.http_f_body, string());
                    resp_fields.insert(self.builtins.http_f_headers, dict(string(), string()));
                    resp_fields.insert(self.builtins.http_f_status, int());
                    Ok(Ty::Record(resp_fields, RowTail::Closed))
                } else if args.is_empty() && self.interner.resolve(name) == Some("Response") {
                    // `Ipe.Http.Server.Response` is a record alias
                    // `{ status : Int, body : String, headers : Dict String
                    // String, contentType : String }` (reference
                    // `Ipê/Http/Server.ipe:66`). Expand structurally — same
                    // mechanism as `HttpResponse` above — so a handler can build
                    // it as a record literal and read fields off it.
                    let mk = |n: Symbol| Ty::Con {
                        module: Vec::new(),
                        name: n,
                        args: Vec::new(),
                    };
                    let string = || mk(self.builtins.string);
                    let int = || mk(self.builtins.int);
                    let dict = |k: Ty, v: Ty| Ty::Con {
                        module: Vec::new(),
                        name: self.builtins.dict,
                        args: vec![k, v],
                    };
                    let mut resp_fields = BTreeMap::new();
                    resp_fields.insert(self.builtins.http_f_body, string());
                    resp_fields.insert(self.builtins.server_f_content_type, string());
                    resp_fields.insert(self.builtins.http_f_headers, dict(string(), string()));
                    resp_fields.insert(self.builtins.http_f_status, int());
                    Ok(Ty::Record(resp_fields, RowTail::Closed))
                } else if args.is_empty() && self.interner.resolve(name) == Some("Migration") {
                    // `Ipe.Db.Migration` is a record alias
                    // `{ name : String, sql : String }`. Expand structurally so a
                    // program can build migrations as record literals in a
                    // `List Migration`.
                    let mk = |n: Symbol| Ty::Con {
                        module: Vec::new(),
                        name: n,
                        args: Vec::new(),
                    };
                    let string = || mk(self.builtins.string);
                    let mut m_fields = BTreeMap::new();
                    m_fields.insert(self.builtins.migration_f_name, string());
                    m_fields.insert(self.builtins.migration_f_sql, string());
                    Ok(Ty::Record(m_fields, RowTail::Closed))
                } else {
                    // Non-Task constructor: recurse into type arguments.
                    let args = args
                        .into_iter()
                        .map(|a| self.normalize_annotation_ty(a, span))
                        .collect::<DResult<Vec<_>>>()?;
                    Ok(Ty::Con { module, name, args })
                }
            }
            Ty::Fun(a, b) => {
                let a = self.normalize_annotation_ty(*a, span)?;
                let b = self.normalize_annotation_ty(*b, span)?;
                Ok(Ty::Fun(Box::new(a), Box::new(b)))
            }
            Ty::Tuple(elems) => {
                let elems = elems
                    .into_iter()
                    .map(|e| self.normalize_annotation_ty(e, span))
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Ty::Tuple(elems))
            }
            Ty::Record(fields, tail) => {
                let fields = fields
                    .into_iter()
                    .map(|(k, v)| self.normalize_annotation_ty(v, span).map(|v| (k, v)))
                    .collect::<DResult<_>>()?;
                Ok(Ty::Record(fields, tail))
            }
            // Leaf types: pass through unchanged.
            other @ (Ty::Var(_) | Ty::Unit) => Ok(other),
        }
    }

    /// Check whether `ty` is the built-in `Error` type — a nullary type
    /// constructor named `"Error"`.  The module path is intentionally ignored so
    /// both bare `Error` and fully-qualified `Ipe.Error.Error` are accepted.
    pub(crate) fn is_error_ty(&self, ty: &Ty) -> bool {
        matches!(
            ty,
            Ty::Con { name, args, .. } if *name == self.builtins.error && args.is_empty()
        )
    }

    /// Constrain a reference to a top-level binding. A typed binding is
    /// instantiated fresh (flex) at this use site so it unifies against its own
    /// concrete arguments without pinning the binding's other call sites, and the
    /// alpha-renaming map is recorded for the post-solve super-type obligation
    /// check. An untyped binding resolves to its shared monomorphic variable; a
    /// name that is not a binding of this module stays fully flexible.
    ///
    /// `module` is the **home** module path carried by the `VarTopLevel` node —
    /// i.e. the path of the module that *declares* the binding, not the module
    /// that *uses* it.  Using this path as part of the lookup key (see
    /// [`Builder::top_level`]) ensures that a `Lib.helper` reference resolves to
    /// `Lib.helper`'s own annotation type even when a same-named `Main.helper`
    /// exists in the merged def list.
    pub(crate) fn constrain_var_top_level(
        &mut self,
        module: &[Symbol],
        name: Symbol,
        span: Span,
    ) -> DResult<VarId> {
        let key = (module.to_vec(), name);
        if let Some(ty) = self.top_level.get(&key).cloned() {
            let (var, vars) = self.instantiate_tracked(&ty)?;
            self.scheme_apps.push(SchemeApp {
                home: module.to_vec(),
                name,
                vars,
                span,
            });
            // A reference to a wildcard-`any`-return binding: record this use's
            // instantiated arrow so [`Self::tie_wildcard_any_uses_to_bodies`]
            // (after all defs are constrained) ties its result to the binding's
            // body — undoing the wildcard severance so the body's real type
            // reaches this use site.
            if self.wildcard_any_return_bindings.contains(&key) {
                self.wildcard_any_use_results.push((var, key));
            }
            Ok(var)
        } else if let Some(v) = self.untyped.get(&key).copied() {
            if key.0 == self.current_home {
                // Same-module: still the one shared monomorphic var — an
                // untyped binding is monomorphic *within its home module*
                // (matches the reference's `CLocal` semantics exactly; see
                // `untyped_polymorphic_use_at_two_types_is_rejected`).
                Ok(v)
            } else {
                // Cross-module: isolate this reference behind its own fresh
                // placeholder instead of sharing the binding's program-wide
                // var. `promote_untyped_boundaries` (in `lib.rs`, post-solve)
                // discharges it against the source binding's generalized
                // scheme, once that scheme exists.
                let placeholder = self.flex()?;
                self.pending_instantiations.push(PendingInstantiation {
                    source: key,
                    placeholder,
                    use_home: self.current_home.clone(),
                    span,
                });
                Ok(placeholder)
            }
        } else {
            Err(Diagnostic::CompilerBug {
                where_: "ipe_types::constrain_var_top_level",
                detail: format!(
                    "unknown top-level binding (symbol {}); \
                     post-link every name must be in top_level or untyped",
                    name.as_raw()
                ),
            })
        }
    }

    /// The Ipê `comparable`-key obligation a kernel's element/key variable
    /// carries, keyed off the resolved [`StdlibKernel`] id via its
    /// `decl().qualifier` (parse-once — never a re-inspected module string).
    /// `Set`'s element is keyed by `BTreeSet` (`Ord`) and `Dict`'s key by a
    /// determinism-sorted `HashMap` (`Hash + Eq + Ord`); the obligation is
    /// attached to raw scheme-variable 0, the element/key in every `Set` /
    /// `Dict` kernel scheme.
    pub(crate) fn key_obligation_for(k: StdlibKernel) -> Option<TyBounds> {
        match k.decl().qualifier {
            "Set" => Some(TyBounds::set_elem()),
            "Dict" => Some(TyBounds::dict_key()),
            // `Ipe.Cache`'s key variable is raw scheme-var 0 in `get` /
            // `put` / `remove` (`Int -> k -> …`), and the runtime scans keys by
            // `PartialEq` (`cache_get`/`cache_put`/`cache_remove` bound
            // `K: PartialEq`). Attaching the EQ obligation lifts `PartialEq`
            // onto the emitted `Ipe.Cache` wrapper's key type parameter. The
            // key-less kernels (`newRaw`/`clear`/`size`/`stats`) have no
            // scheme-var 0, so the `vars.get(&0)` tie is a no-op for them.
            "Cache" => Some(TyBounds::eq()),
            _ => None,
        }
    }

    /// The raw scheme-var id of the CALLBACK-RESULT slot of a `Maybe`/`Result`
    /// higher-order kernel — the variable that must not itself instantiate to
    /// a function ([`TyBounds::hof_kernel_result`]).
    ///
    /// Slot ids follow each kernel's scheme in [`Self::stdlib_scheme`] and are
    /// asserted against those schemes by
    /// `hof_result_slots_match_scheme_shapes` (this module's tests): `map`'s
    /// `(a -> b)` result `b` is `var(1)`; `mapError`'s `(e -> f)` result `f`
    /// is `var(1)`; `mapN`'s `(a -> … -> v)` final result `v` is `var(N)`;
    /// `andMap`'s payload `Con (a -> b)` result `b` is `var(1)`.
    ///
    /// Deliberately EXCLUDED, with reasons:
    /// * `MaybeAndThen` / `ResultAndThen` / `ResultTraverse` — their callback
    ///   results are `Con`-headed in the scheme itself (`a -> Maybe b`, `a ->
    ///   Result e b`), so a curried callback is already a plain type mismatch
    ///   (`Fun` vs `Con`); there is no bare var for an arrow to escape into.
    /// * `MaybeWithDefault` / `ResultWithDefault` / `MaybeCombine` /
    ///   `ResultCombine` — no callback is applied by the kernel; a
    ///   function-valued payload flows through by value in its (consistently
    ///   flattened) representation, which is sound.
    /// * `Task` / `Cmd` / `Sub` / `Decoder` kernels — out of scope:
    ///   their heads are exempted from the ctor-payload region gate
    ///   (`is_opaque_boxed_wrapper`), so any curried-callback
    ///   hazard there is tracked separately (the
    ///   `Decoder` family in particular must NOT be gated — its runtime has
    ///   genuine `curry1..curry10` currying support the applicative decoder
    ///   pipeline depends on).
    pub(crate) const fn hof_result_slot_for(k: StdlibKernel) -> Option<u32> {
        use StdlibKernel as K;
        match k {
            K::MaybeMap | K::ResultMap | K::ResultMapError | K::MaybeAndMap | K::ResultAndMap => {
                Some(1)
            }
            K::MaybeMap2 | K::ResultMap2 => Some(2),
            K::MaybeMap3 | K::ResultMap3 => Some(3),
            K::MaybeMap4 | K::ResultMap4 => Some(4),
            K::MaybeMap5 | K::ResultMap5 => Some(5),
            _ => None,
        }
    }

    /// The type of a kernel reference (`Math.min`, `Set.insert`, …).
    ///
    /// Most kernels take the declarative scheme from [`Self::stdlib_scheme`] via
    /// `instantiate`. Two families instead mint super-typed obligations so a
    /// generic use lifts the matching Rust trait bound onto its annotation
    /// skolem and a non-comparable argument fails closed at type-check:
    ///
    /// * `Math.min` / `Math.max` — `Comparable a => a -> a -> a`: the shared
    ///   variable carries the ORDERING obligation, exactly as the `< > <= >=`
    ///   operators and the user-fn `maxOf` do, so a generic use emits Rust
    ///   `T: PartialOrd` and a function / record argument is rejected rather than
    ///   emitting an unbounded `math_min<T>(…)` that `cargo` rejects.
    /// * `Set` / `Dict` kernels — the element / key (raw scheme-variable 0 in
    ///   every Set / Dict kernel) carries the Ipê `comparable`-key obligation
    ///   ([`Self::key_obligation_for`]). The base scheme (now in
    ///   [`Self::stdlib_scheme`]) is instantiated, then variable 0 is tied to a
    ///   fresh super-typed variable carrying that obligation, so a
    ///   non-comparable element / key (record, ADT, function) fails closed
    ///   instead of emitting an unbounded `set_insert::<T>` / `dict_insert::<T>`
    ///   call `cargo` rejects, and a generic `a -> Set a` lifts `Ord` (Set) /
    ///   `Hash + Eq + Ord` (Dict) onto its annotation skolem (see `bounds_for`).
    ///   This is also more conservative than Ipê's runtime, which keys a Set /
    ///   Dict on a stringified value.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn constrain_var_kernel(
        &mut self,
        id: Option<StdlibKernel>,
        module: Symbol,
        name: Symbol,
        span: Span,
    ) -> DResult<VarId> {
        // ── Obligation pre-checks (keyed off the resolved `id`,
        //    not a re-inspected module string). They live OUTSIDE the scheme
        //    tables and must fire BEFORE the registry/legacy delegation, so the
        //    bounded super-var reaches the caller instead of the bare base
        //    scheme now sitting in `stdlib_scheme`. ──
        if let Some(k) = id {
            // `Math.min` / `Math.max`: `Comparable a => a -> a -> a`. The bounded
            // super-var (reused across BOTH arrow argument positions AND the
            // result) is what rejects `Math.min f g` / `Math.min recA recB`
            // (`golden_m4c_math_gate`). This is a DIRECT-build bounded
            // scheme, NOT `stdlib_scheme` + a tie, because min/max's base scheme
            // has three independent `var(0)`s and the gate needs all three tied
            // to one bounded var.
            if matches!(k, StdlibKernel::MathMin | StdlibKernel::MathMax) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let inner = self.structure(FlatType::Fun(s, s))?;
                return self.structure(FlatType::Fun(s, inner));
            }
            // `Basics.clamp lo hi x : comparable -> comparable -> comparable ->
            // comparable`. Same ORDERING obligation as min/max, but arity 3:
            // ONE bounded super-var reused across all three argument positions
            // AND the result, so `clamp recA recB recC` (records / functions /
            // ADTs) fails closed instead of emitting an unbounded
            // `basics_clamp::<T>` that `cargo` rejects. DIRECT-build (not
            // `stdlib_scheme` + tie) because the base scheme has three
            // independent `var(0)`s that must collapse to one bounded var.
            if matches!(k, StdlibKernel::BasicsClamp) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let inner1 = self.structure(FlatType::Fun(s, s))?;
                let inner2 = self.structure(FlatType::Fun(s, inner1))?;
                return self.structure(FlatType::Fun(s, inner2));
            }
            // ── Basics numerics ────────────────────────────────────────
            // `negate / abs : number a => a -> a`. SUB obligation (Number
            // super-type — same as the unary-minus operator). A function / record
            // argument fails closed (T0001) before reaching a runtime that would
            // panic. Base scheme for the totality gate is in `stdlib_scheme`.
            if matches!(k, StdlibKernel::BasicsNegate | StdlibKernel::BasicsAbs) {
                let s = self.super_var(TyBounds::sub(), span)?;
                return self.structure(FlatType::Fun(s, s));
            }
            // `Store.add / .sub / .mul : number a => a -> a -> a` — the
            // arithmetic projection operators.  ONE Number-bounded super-var is
            // reused across both argument positions AND the result, so a
            // non-numeric operand (String / Bool / record / function) fails
            // closed (T0001) instead of emitting an unbounded scheme, and the
            // two operands must share the same numeric type.  The obligation is
            // the same one `+` / `-` / `*` mint (ADD / SUB / MUL).  DIRECT-build
            // (not `stdlib_scheme` + tie) so all three positions collapse to the
            // one bounded var.
            if let Some(bound) = match k {
                StdlibKernel::StoreAdd => Some(TyBounds::add()),
                StdlibKernel::StoreSub => Some(TyBounds::sub()),
                StdlibKernel::StoreMul => Some(TyBounds::mul()),
                _ => None,
            } {
                let s = self.super_var(bound, span)?;
                let inner = self.structure(FlatType::Fun(s, s))?;
                return self.structure(FlatType::Fun(s, inner));
            }
            // `min / max : comparable a => a -> a -> a` — same Comparable (Ord)
            // obligation as `Math.min` / `Math.max`. DIRECT-build (not
            // `stdlib_scheme` + tie) so all three positions collapse to ONE
            // bounded super-var, rejecting function / record arguments closed.
            if matches!(k, StdlibKernel::BasicsMin | StdlibKernel::BasicsMax) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let inner = self.structure(FlatType::Fun(s, s))?;
                return self.structure(FlatType::Fun(s, inner));
            }
            // `compare : comparable a => a -> a -> Order`. Direct-build
            // (not stdlib_scheme + tie): both argument positions share one
            // Ord-bounded super-var; the return is the monomorphic Order type.
            if matches!(k, StdlibKernel::BasicsCompare) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let order_var = self.structure(FlatType::Con {
                    module: Vec::new(),
                    name: self.builtins.order,
                    args: Vec::new(),
                })?;
                let inner = self.structure(FlatType::Fun(s, order_var))?;
                return self.structure(FlatType::Fun(s, inner));
            }
            // ── end Basics numerics ────────────────────────────────────
            // `List.sum : number a => List a -> a` / `List.product`. The list
            // element and the result share ONE number-bounded super-var (ADD for
            // sum, MUL for product — the same obligation `+` / `*` mint), so a
            // non-numeric element fails closed instead of emitting an unbounded
            // `list_sum::<T>`. Direct-build (not `stdlib_scheme` + tie) so both
            // the element and the result collapse to one bounded var.
            if matches!(k, StdlibKernel::ListSum | StdlibKernel::ListProduct) {
                let bound = if matches!(k, StdlibKernel::ListSum) {
                    TyBounds::add()
                } else {
                    TyBounds::mul()
                };
                let s = self.super_var(bound, span)?;
                let list_s = self.list_var(s)?;
                return self.structure(FlatType::Fun(list_s, s));
            }
            // `List.maximum / minimum : comparable a => List a -> Maybe a`. The
            // element carries the ORDERING obligation (same as `Math.min/max`);
            // the result is `Maybe a` over that bounded var. Direct-build so the
            // element and the Maybe payload share the one bounded super-var.
            if matches!(k, StdlibKernel::ListMaximum | StdlibKernel::ListMinimum) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let list_s = self.list_var(s)?;
                let maybe_s = self.structure(FlatType::Con {
                    module: Vec::new(),
                    name: self.builtins.maybe,
                    args: vec![s],
                })?;
                return self.structure(FlatType::Fun(list_s, maybe_s));
            }
            // `List.sort : comparable a => List a -> List a`. The element carries
            // the ORDERING obligation; input and output share the one bounded
            // super-var. Direct-build (not `stdlib_scheme` + tie).
            if matches!(k, StdlibKernel::ListSort) {
                let s = self.super_var(TyBounds::ord(), span)?;
                let list_s = self.list_var(s)?;
                let list_s2 = self.list_var(s)?;
                return self.structure(FlatType::Fun(list_s, list_s2));
            }
            // `Basics.toString : a -> String`. The argument carries the
            // STRINGIFY obligation (a bounded super-var → Rust `IpeStringify`):
            // a scalar / record / ADT satisfies it, a bare function (or a value
            // nesting one) fails CLOSED at type-check rather than emitting an
            // unbounded `basics_to_string::<T>` that `cargo` rejects. Direct-build
            // (not stdlib_scheme + tie): only the argument position is bounded.
            // This is the shared lever for the whole Stringify-bounded family
            // (Log.*With / Debug.toString) — wire those the same way.
            if matches!(
                k,
                StdlibKernel::BasicsToString | StdlibKernel::ErrorToString
            ) {
                let s = self.super_var(TyBounds::show(), span)?;
                let string_ty = self.string_var()?;
                return self.structure(FlatType::Fun(s, string_ty));
            }
            // Dict / Set element-key `comparable` obligation. The base
            // scheme is relocated into `stdlib_scheme`; we instantiate
            // it and tie key-position raw var 0 to a bounded super-var. Only
            // key-position `var(0)` carries the bound, so this is `stdlib_scheme`
            // + a tie (unlike min/max's direct-build shape above).
            if let Some(bound) = Self::key_obligation_for(k) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let Some(&key_var) = vars.get(&0) {
                    let s = self.super_var(bound, span)?;
                    self.eq(span, key_var, s);
                }
                // `Set.map : (a -> b) -> Set a -> Set b` — the RESULT element
                // `b` (raw scheme-var 1) also backs a `BTreeSet<b>`, so it
                // carries the same `set_elem` (Ord) obligation as the source
                // element. Without this a generic `Set.map` would emit an
                // unbounded `set_map::<A, B>` that `cargo` rejects (B: Ord unmet).
                if matches!(k, StdlibKernel::SetMap)
                    && let Some(&res_var) = vars.get(&1)
                {
                    let s = self.super_var(bound, span)?;
                    self.eq(span, res_var, s);
                }
                return Ok(var);
            }
            // `Db.exec` / `Db.query` / `Db.queryDecode`: the params-LIST
            // ELEMENT (raw scheme-var 0 for `exec`/`query`; var 1 for
            // `queryDecode`, whose var 0 is the decoder's result type — see
            // the scheme comments above) carries the SQL-bind-parameter
            // obligation. Same `stdlib_scheme` + tie shape as the Set/Dict
            // key obligation directly above: only the params-element
            // position is bounded, so a generic wrapper around `Db.exec` /
            // `Db.query` (`Database.exec label queryStr args` in
            // `examples/17-ipemon`) lifts `Into<SqlParam>` onto its own
            // emitted Rust generic (closing the E0277 half), and an
            // empty-list call site whose element type is otherwise
            // completely unconstrained defaults to `SqlValue` at solve time
            // instead of the wildcard-`any` fallback (closing the E0283
            // half — see the `sql_param` arm of the numeric-defaulting loop
            // in `crate::lib`), rather than emitting a bare `Vec::new()`
            // `cargo` cannot infer.
            if matches!(
                k,
                StdlibKernel::DbExec
                    | StdlibKernel::DbQuery
                    | StdlibKernel::DbQueryDecode
                    | StdlibKernel::DbConnQueryDecode
            ) {
                // The params-list element var is index 1 for both `queryDecode`
                // shapes (they carry a decoder var 0 ahead of it), index 0 for the
                // bare `exec`/`query`.
                let raw_idx = u32::from(matches!(
                    k,
                    StdlibKernel::DbQueryDecode | StdlibKernel::DbConnQueryDecode
                ));
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let Some(&params_var) = vars.get(&raw_idx) {
                    let s = self.super_var(TyBounds::sql_param(), span)?;
                    self.eq(span, params_var, s);
                }
                return Ok(var);
            }
            // Higher-order-kernel callback-result obligation
            // (primary/Tier-2 mechanism — see
            // `docs/adr/0016-andmap-arity-gate-type-obligation.md`).
            // Every `Maybe`/`Result` higher-order kernel FULLY APPLIES its
            // callback at runtime (`FnOnce(..) -> R` with an exact arity),
            // while the IR flattens a curried Ipê function into one
            // multi-parameter `Fun` — so a callback with residual arity (its
            // final result var instantiates to another arrow) has no sound
            // lowering and would reach `cargo build` as E0277/E0308. Tie the
            // callback's final-result raw scheme-var (see
            // [`Self::hof_result_slot_for`]) to a fresh super-typed variable
            // carrying the `hof_kernel_result` obligation — same
            // `stdlib_scheme` + tie shape as the Dict/Set key obligation
            // above, so this is a genuine TYPE-LEVEL check that survives
            // arbitrary Ipê-level aliasing (direct call, piped, `let`-bound,
            // bare-value re-export, higher-order argument, record-field
            // extraction, import alias) by construction — the obligation is
            // attached to the union-find variable `constrain_var_kernel`
            // mints for THIS kernel reference, not to any particular AST
            // shape a later use might take.
            if let Some(slot) = Self::hof_result_slot_for(k) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let Some(&callback_result_var) = vars.get(&slot) {
                    let s = self.super_var(TyBounds::hof_kernel_result(), span)?;
                    self.eq(span, callback_result_var, s);
                }
                return Ok(var);
            }
            // `Log.*With : String -> List a -> Task Error ()` — the attr-list
            // ELEMENT `a` carries the STRINGIFY obligation. Same
            // `stdlib_scheme` + tie shape as Dict/Set: instantiate the base
            // scheme and tie its list-element `var(0)` to a Show super-var, so a
            // non-showable element (a function) fails closed at type-check.
            if matches!(
                k,
                StdlibKernel::LogInfoWith
                    | StdlibKernel::LogDebugWith
                    | StdlibKernel::LogWarnWith
                    | StdlibKernel::LogErrorWith
            ) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let Some(&elem_var) = vars.get(&0) {
                    let s = self.super_var(TyBounds::show(), span)?;
                    self.eq(span, elem_var, s);
                }
                return Ok(var);
            }
            // `Debug.log : String -> a -> a` — the value `a` (shared by the
            // argument and result, raw scheme-var 0) carries the STRINGIFY
            // obligation (the runtime stringifies it through the same
            // `IpeStringify` path as `Basics.toString`). Same `stdlib_scheme` +
            // tie shape as `Log.*With`: tying the ONE super-var to both
            // positions keeps `Debug.log Int 5` (concrete, satisfies `show`)
            // accepted while a bare-function value fails closed — no spurious
            // IPE-L0108 for a well-typed showable value.
            if matches!(k, StdlibKernel::DebugLog) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let Some(&value_var) = vars.get(&0) {
                    let s = self.super_var(TyBounds::show(), span)?;
                    self.eq(span, value_var, s);
                }
                return Ok(var);
            }
            // `Web.app` — post-solve routed-Web check.
            //
            // The open-record cfg scheme for K::WebApp is shared by both routed
            // apps (Model has a `page : Page` field) and non-routed apps (Model
            // has no `page` field).  We cannot express the conditional
            // `Model.page ≡ notFound` constraint at build time because a blanket
            // `var(0) ≡ { page : var(2) | ρ }` would break every non-routed
            // app whose Model has no `page` field.
            //
            // Instead: instantiate the scheme with `instantiate_tracked`, record
            // the Model var (var index 0) and notFound var (var index 2), then
            // push a `RoutedWebCheck` so `resolve_routed_web_checks` can run
            // the gate after the HM solver settles.
            if matches!(k, StdlibKernel::WebApp | StdlibKernel::WebEmbed) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let (Some(&model_var), Some(&not_found_var)) = (vars.get(&0), vars.get(&2)) {
                    self.routed_web_checks.push(RoutedWebCheck {
                        model_var,
                        not_found_var,
                        span,
                    });
                }
                return Ok(var);
            }
            // `Web.route` — per-route page witness.
            //
            // The scheme types the page-builder argument with var(1) DISTINCT
            // from the result's page var(0): the argument is EITHER a nullary
            // page value (`Web.route "/" HomePage`) OR a params-consuming
            // constructor (`Web.route "/apps/:slug" AppDetailPage` — type
            // `String -> Page`).  That disjunction is not expressible as a
            // plain HM constraint, so — like `RoutedWebCheck` above — the
            // relation is deferred: record both instantiated vars and push a
            // `RouteWitnessCheck`; `resolve_route_witness_checks` peels the
            // builder's settled leading arrows and unifies the resulting page
            // type with var(0) after the main solve.
            if matches!(k, StdlibKernel::WebRoute) {
                let ty = self.stdlib_scheme(k).ok_or(Diagnostic::Lower {
                    span,
                    msg: LowerError::Unsupported(Feature::Kernels),
                })?;
                let (var, vars) = self.instantiate_tracked(&ty)?;
                if let (Some(&page_var), Some(&builder_var)) = (vars.get(&0), vars.get(&1)) {
                    self.route_witness_checks.push(RouteWitnessCheck {
                        builder_var,
                        page_var,
                        span,
                    });
                }
                return Ok(var);
            }
        }
        // ── Parse-once registry lookup ──
        //
        // `stdlib_scheme` is TOTAL over the reachable kernel set and
        // WILDCARD-FREE, so every reachable kernel resolves via the
        // `StdlibKernel` id. There is no legacy string-keyed `kernel_ty`
        // table carrying a `Ty::Var(u32::MAX)` exit-0 sentinel for un-typed
        // kernels. A `None` id (FFI `Rust.*`) or an excluded bucket
        // (`WebAppRouted` — unlowered) misses the registry and is
        // fail-closed with IPE-L0108 (loud) via `kernel_scheme_or_unsupported`,
        // never silently typed as a free variable that `cargo` later rejects.
        let _ = (module, name); // retained for diagnostics
        // Route through `resolve_scheme`, not `stdlib_scheme` directly, so a
        // kernel carrying a structural `TyShape` resolves via the interpreter and
        // one without a shape resolves through the table — a single adapter, so
        // the two paths can never resolve to different types.
        let registry = id.and_then(|k| self.resolve_scheme(SchemeKey(k)));
        let ty = Self::kernel_scheme_or_unsupported(registry, None, span)?;
        self.instantiate(&ty)
    }

    /// Combine the parse-once registry scheme (`id` path) with the legacy
    /// string-table scheme, failing closed with IPE-L0108 (`Feature::Kernels`,
    /// the same shape lower raises at `lower_callee`) when NEITHER supplies a
    /// type. Extracted as a pure fn so the fail-closed arm is unit-testable
    /// independently of the (currently total) legacy table — see
    /// `both_miss_is_fail_closed`.
    pub(crate) fn kernel_scheme_or_unsupported(
        registry: Option<Ty>,
        legacy: Option<Ty>,
        span: Span,
    ) -> DResult<Ty> {
        registry.or(legacy).ok_or(Diagnostic::Lower {
            span,
            msg: LowerError::Unsupported(Feature::Kernels),
        })
    }

    #[allow(clippy::too_many_lines)] // one arm per canonical expression form
    pub(crate) fn constrain_expr(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        e: &canon::Expr,
    ) -> DResult<VarId> {
        let span = e.span;
        let var = match &e.value {
            // An integer literal is `Number`-polymorphic (Elm/Ipê `number`): it
            // may resolve to `Int` OR `Float` depending on context, and defaults
            // to `Int` when the program never pins it (the post-solve defaulting
            // loop closes an unpinned `Super { Number }` to `Int`).  This lets
            // `pct 100` — where `pct : Float -> Length` — accept the literal `100`
            // as a `Float`, matching the reference compiler.  A *float* literal
            // (`1.6`) is concretely `Float`, never `Int` (Elm keeps `1.6 : Float`
            // distinct from the polymorphic `number`).
            canon::Expr_::Int(_) => self.super_var(TyBounds::add(), span)?,
            canon::Expr_::Float(_) => self.float_var()?,
            canon::Expr_::Str(_) => self.string_var()?,
            canon::Expr_::PathLit(_) => self.path_var()?,
            canon::Expr_::CustomElementCtor(_) => self.custom_element_var()?,
            canon::Expr_::Char(_) => self.char_var()?,
            canon::Expr_::Unit => self.structure(FlatType::Unit)?,
            canon::Expr_::VarLocal(s) => match local.get(s) {
                Some(v) => *v,
                None => {
                    return Err(Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: format!(
                            "unbound local `{}`",
                            self.interner.resolve(*s).unwrap_or("<unknown symbol>")
                        ),
                    });
                }
            },
            canon::Expr_::VarTopLevel { module, name } => {
                self.constrain_var_top_level(module, *name, span)?
            }
            canon::Expr_::VarKernel { id, module, name } => {
                // the pre-resolved `id` selects the parse-once
                // registry scheme (`stdlib_scheme`) for migrated families,
                // falling back to the legacy symbol-keyed table otherwise.
                self.constrain_var_kernel(*id, *module, *name, span)?
            }
            canon::Expr_::ForeignCall { args, .. } => {
                // A foreign wrapper call is the annotation-trusted boundary:
                // the enclosing FfiInterface binding is REQUIRED to carry a
                // full annotation (canon fails closed otherwise), and that
                // annotation pins every parameter and the result. Arguments
                // are constrained so their vars exist for the lowerer's
                // region map; the call's own type is a fresh flexible var the
                // annotation immediately determines.
                for a in args {
                    self.constrain_expr(local, a)?;
                }
                self.flex()?
            }
            canon::Expr_::VarCtor {
                home,
                type_name,
                name,
                ..
            } => self.constrain_var_ctor(home, *type_name, *name)?,
            canon::Expr_::Call(callee, args) => {
                let callee_var = self.constrain_expr(local, callee)?;
                // Each argument gets a FRESH param var rather than flowing its
                // own var straight into the callee's arrow shape. Two payoffs:
                //  1. the callee-vs-shape constraint below is solved FIRST, so
                //     each param var adopts the callee's DECLARED param type;
                //  2. the per-arg constraint then unifies found=actual-arg
                //     against expected=declared-param AT THE ARG'S SPAN —
                //     `Task.fail "str"` reads "expected Error, found String",
                //     never the inversion (and blames the argument, not the
                //     callee name).
                // A non-function callee still reports found=callee's type,
                // expected=`a -> b` via the callee-vs-shape constraint.
                let mut arg_pairs = Vec::with_capacity(args.len());
                for a in args {
                    let arg_var = self.constrain_expr(local, a)?;
                    let param_var = self.flex()?;
                    // The callee's declared slot is exactly the type this
                    // argument position expects: after the callee-vs-shape
                    // constraint solves, `param_var` adopts the declared param
                    // type, so completion at this span offers only candidates
                    // whose type unifies with the declared parameter.
                    self.record_expected(a.span, param_var);
                    arg_pairs.push((a.span, arg_var, param_var));
                }
                let ret = self.flex()?;
                // Fold a right-associative arrow over the fresh param vars:
                // p0 -> p1 -> … -> ret.
                let mut fun_shape = ret;
                for (_, _, param_var) in arg_pairs.iter().rev() {
                    fun_shape = self.structure(FlatType::Fun(*param_var, fun_shape))?;
                }
                // Order matters: callee-vs-shape first (see above).
                self.eq(callee.span, callee_var, fun_shape);
                for (arg_span, arg_var, param_var) in arg_pairs {
                    self.eq(arg_span, arg_var, param_var);
                }
                ret
            }
            canon::Expr_::Case(scrut, branches) => self.constrain_case(local, scrut, branches)?,
            canon::Expr_::Lambda(params, body) => self.constrain_lambda(local, params, body)?,
            canon::Expr_::Binop { func, lhs, rhs, .. } => {
                self.constrain_binop(local, *func, lhs, rhs)?
            }
            canon::Expr_::Let(bindings, body) => {
                // Sequential, monomorphic `let`: each binding's value is
                // constrained against the scope built so far, and its name binds
                // to that value's variable for the bindings that follow and the
                // `in` body. The whole `let`'s type is the body's type. It does
                // not generalise let-bound names — no let-polymorphism.
                let mut let_local = local.clone();
                for b in bindings {
                    let bv = self.constrain_expr(&let_local, &b.body)?;
                    // The binder may be a plain name or an irrefutable destructure
                    // (tuple / record); `constrain_pattern` ties the binder's
                    // shape to the value's type and binds every leaf variable.
                    self.constrain_pattern(&mut let_local, &b.pat, bv)?;
                }
                self.constrain_expr(&let_local, body)?
            }
            canon::Expr_::If(branches, else_expr) => {
                // Every condition is `Bool`; every branch and the final `else`
                // unify to one shared result type, which is the whole `if`'s
                // type. Mirrors `Ipe.Type.Constrain.Expression.constrainIf`.
                let result = self.flex()?;
                for (cond, body) in branches {
                    let cond_var = self.constrain_expr(local, cond)?;
                    let want_bool = self.bool_var()?;
                    // A condition expects `Bool`; a branch body expects the
                    // shared `if` result type.
                    self.record_expected(cond.span, want_bool);
                    self.eq(cond.span, cond_var, want_bool);
                    let body_var = self.constrain_expr(local, body)?;
                    self.record_expected(body.span, result);
                    self.eq(body.span, body_var, result);
                }
                let else_var = self.constrain_expr(local, else_expr)?;
                self.record_expected(else_expr.span, result);
                self.eq(else_expr.span, else_var, result);
                result
            }
            canon::Expr_::Tuple(elems) => {
                // A tuple's type is the product of its elements' types, each
                // constrained independently. Mirrors
                // `Ipe.Type.Constrain.Expression`'s tuple arm.
                let mut elem_vars = Vec::with_capacity(elems.len());
                for elem in elems {
                    elem_vars.push(self.constrain_expr(local, elem)?);
                }
                self.structure(FlatType::Tuple(elem_vars))?
            }
            canon::Expr_::List(elems) => self.constrain_list(local, elems)?,
            canon::Expr_::Cons(head, tail) => self.constrain_cons(local, head, tail)?,
            canon::Expr_::Record(fields) => self.constrain_record(local, fields)?,
            canon::Expr_::Access(record, field) => {
                self.constrain_access(local, record, *field, span)?
            }
            canon::Expr_::Update(base, fields) => {
                self.constrain_update(local, base, fields, span)?
            }
        };
        self.regions.insert((self.current_home.clone(), span), var);
        Ok(var)
    }

    /// Constrain a lambda `\p0 p1 ... -> body`. Each parameter gets a fresh
    /// flexible variable bound in the body's scope; the body is constrained
    /// there. The lambda's type is the right-nested arrow `p0 -> p1 -> … -> body`,
    /// so a surrounding `Call` unifies its callee against exactly this shape.
    /// Mirrors `Ipe.Type.Constrain.Expression`'s lambda arm.
    pub(crate) fn constrain_lambda(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        params: &[canon::Pattern],
        body: &canon::Expr,
    ) -> DResult<VarId> {
        let mut lam_local = local.clone();
        let mut param_vars = Vec::with_capacity(params.len());
        for p in params {
            let v = self.flex()?;
            self.constrain_pattern(&mut lam_local, p, v)?;
            // Record each lambda param's region so the lowerer can source a
            // record-param's complete field set from its solved type (one path
            // shared with the typed-def sites).  Keyed by `(current_home, span)`
            // to prevent cross-module span collisions.
            self.regions.insert((self.current_home.clone(), p.span), v);
            param_vars.push(v);
        }
        let mut arrow = self.constrain_expr(&lam_local, body)?;
        for pv in param_vars.into_iter().rev() {
            arrow = self.structure(FlatType::Fun(pv, arrow))?;
        }
        Ok(arrow)
    }

    /// Constrain a record literal `{ name = value, ... }`. Its type is the
    /// closed record `{ name : <field type>, ... }`, each field value
    /// constrained independently. Canonicalisation has already rejected a
    /// duplicate field name, so the resulting field map is exact.
    ///
    /// User-written record literals are always **closed** — they carry an
    /// `EmptyRecord` tail so the unifier rejects extra fields on either side.
    pub(crate) fn constrain_record(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        fields: &[(Symbol, canon::Expr)],
    ) -> DResult<VarId> {
        let mut field_vars = BTreeMap::new();
        for (name, value) in fields {
            let v = self.constrain_expr(local, value)?;
            field_vars.insert(*name, v);
        }
        let ext = self.empty_record_tail()?;
        self.structure(FlatType::Record(field_vars, ext))
    }

    /// Tie each reference to a wildcard-`any`-return binding to that binding's
    /// body result, so the body's settled type flows to every use site — closing
    /// the wildcard severance at its root. Run once EVERY def is constrained
    /// (so all body vars exist and the tie is independent of source order),
    /// before the main solve, so the tied type propagates through the same
    /// unification the use participates in. A `view = <binding>` whose body is
    /// `Html` therefore reaches the shape's `Element` requirement as an ordinary
    /// mismatch (rendered as IPE-T0020), rather than passing ipe and failing
    /// `cargo build`. Covers every indirection — direct reference, `let` alias
    /// chains, eta-expansion — because it is plain unification, not a syntactic
    /// reference walk.
    pub(crate) fn tie_wildcard_any_uses_to_bodies(&mut self) -> DResult<()> {
        let ties = std::mem::take(&mut self.wildcard_any_use_results);
        for (use_arrow, binding) in ties {
            let Some(&body_var) = self.wildcard_any_return_bodies.get(&binding) else {
                continue;
            };
            // Peel BOTH the use's instantiated arrow and the recorded body to
            // their final results, then tie the two result slots. The use arrow
            // is `param0 -> … -> any`; the body is either the applied result
            // (a def written with parameters) OR the same arrow shape (a
            // point-free def, `alias = view`), so peeling both reaches the
            // matching `any`/`Html` slot regardless of the def form or arity.
            let use_result = self.peel_arrow_result(use_arrow)?;
            let body_result = self.peel_arrow_result(body_var)?;
            self.eq(Span::DUMMY, use_result, body_result);
        }
        Ok(())
    }

    /// Follow a variable's settled structure, peeling leading `_ -> rest`
    /// arrows, and return the final non-arrow result. Bounded fuel guards a
    /// pathological cyclic chain.
    pub(crate) fn peel_arrow_result(&mut self, var: VarId) -> DResult<VarId> {
        let mut cur = self.uf.find(var)?;
        let mut fuel: u32 = 1024;
        while fuel > 0 {
            match self.uf.content(cur)? {
                Content::Structure(FlatType::Fun(_, ret)) => cur = self.uf.find(ret)?,
                _ => break,
            }
            fuel -= 1;
        }
        Ok(cur)
    }

    /// Constrain a record field access `record.field`. With closed records (no
    /// row variable), the field cannot be resolved until the record's type
    /// settles, so the access is deferred: a fresh result variable is its region
    /// type now, and [`crate::resolve_field_accesses`] links it to the field's
    /// type after the main solve.
    pub(crate) fn constrain_access(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        record: &canon::Expr,
        field: Symbol,
        span: Span,
    ) -> DResult<VarId> {
        let record_var = self.constrain_expr(local, record)?;
        let result = self.flex()?;
        self.field_accesses.push(FieldAccess {
            record: record_var,
            field,
            result,
            span,
            home: self.current_home.clone(),
        });
        Ok(result)
    }

    /// Constrain a record update `{ base | field = value, ... }`. The result
    /// type is the base record's type (an update copies-and-replaces, changing
    /// no field's type), so the update's region variable *is* the base's. The
    /// field-existence + per-field type checks are deferred — closed records
    /// carry no row variable, so the base's type may not be settled yet —
    /// recorded here and discharged by [`crate::resolve_record_updates`] after
    /// the main solve.
    pub(crate) fn constrain_update(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        base: &canon::Expr,
        fields: &[(Symbol, canon::Expr)],
        span: Span,
    ) -> DResult<VarId> {
        let record_var = self.constrain_expr(local, base)?;
        let mut field_vars = Vec::with_capacity(fields.len());
        for (name, value) in fields {
            let v = self.constrain_expr(local, value)?;
            field_vars.push((*name, v));
        }
        self.record_updates.push(RecordUpdate {
            record: record_var,
            fields: field_vars,
            span,
            home: self.current_home.clone(),
        });
        Ok(record_var)
    }

    /// Constrain a `case scrut of …`: the scrutinee shares one type, every arm
    /// pattern is checked against it, and every arm body unifies to one shared
    /// result — the whole `case`'s type.
    pub(crate) fn constrain_case(
        &mut self,
        local: &BTreeMap<Symbol, VarId>,
        scrut: &canon::Expr,
        branches: &[canon::CaseBranch],
    ) -> DResult<VarId> {
        let scrut_var = self.constrain_expr(local, scrut)?;
        let result = self.flex()?;
        for br in branches {
            let mut br_local = local.clone();
            self.constrain_pattern(&mut br_local, &br.pat, scrut_var)?;
            let body_var = self.constrain_expr(&br_local, &br.body)?;
            // Every arm body expects the shared `case` result type.
            self.record_expected(br.body.span, result);
            self.eq(br.body.span, body_var, result);
        }
        Ok(result)
    }

    /// Constrain a constructor referenced as a value: its scheme instantiated
    /// fresh. A nullary constructor's value type is the enum itself; a payload
    /// constructor's is the curried arrow `field0 -> … -> T vars`. Each reference
    /// instantiates independently, so the same generic constructor used at `Int`
    /// and at `Bool` in one module yields two separately-satisfiable types. A
    /// constructor with no registered scheme (imported, outside the single-module
    /// subset) falls back to the bare enum type, sound for the nullary case.
    pub(crate) fn constrain_var_ctor(
        &mut self,
        home: &[Symbol],
        type_name: Symbol,
        name: Symbol,
    ) -> DResult<VarId> {
        // Same qualified-identity lookup as the pattern site: a constructor
        // referenced as a value resolves against its own declaring module's
        // scheme, never a same-named constructor from another module.
        let key = (home.to_vec(), type_name, name);
        if let Some(scheme) = self.ctors.get(&key).cloned() {
            let (arg_vars, result_var) = self.instantiate_ctor(&scheme)?;
            let mut t = result_var;
            for av in arg_vars.into_iter().rev() {
                t = self.structure(FlatType::Fun(av, t))?;
            }
            Ok(t)
        } else {
            self.con_var(home.to_vec(), type_name, Vec::new())
        }
    }

    /// Constrain a `case` arm pattern against the scrutinee's variable, binding
    /// any pattern variables into `local`.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn constrain_pattern(
        &mut self,
        local: &mut BTreeMap<Symbol, VarId>,
        pat: &canon::Pattern,
        scrut_var: VarId,
    ) -> DResult<()> {
        match &pat.value {
            canon::Pattern_::PAnything => Ok(()),
            // The unit pattern `()` pins the scrutinee to the unit type and binds
            // nothing — the pattern-position counterpart of the unit expression.
            canon::Pattern_::PUnit => {
                let unit = self.structure(FlatType::Unit)?;
                self.eq(pat.span, unit, scrut_var);
                Ok(())
            }
            canon::Pattern_::PVar(s) => {
                local.insert(*s, scrut_var);
                Ok(())
            }
            canon::Pattern_::PCtor {
                home,
                type_name,
                name,
                args,
                ..
            } => {
                // Look up by the canon-resolved `(home, type_name, name)`
                // identity, not the bare name: a same-named constructor in a
                // different module (or type) is a DIFFERENT entry, so this
                // module's own pattern checks against its own declared arity.
                let key = (home.clone(), *type_name, *name);
                if let Some(scheme) = self.ctors.get(&key).cloned() {
                    // A constructor pattern binds exactly its declared fields. A
                    // mismatch (`Just` with no payload, `Node l r` for a three-field
                    // `Node`) is a user error, surfaced as IPE-T0013 rather than
                    // silently constraining a prefix.
                    if args.len() != scheme.arg_tys.len() {
                        return Err(self.ctor_pattern_arity(
                            pat.span,
                            *name,
                            scheme.arg_tys.len(),
                            args.len(),
                        ));
                    }
                    // Instantiate the scheme fresh, tie the result to the
                    // scrutinee, and constrain each payload sub-pattern against its
                    // field's (now use-site) type. Recursing handles a nested
                    // sub-pattern's typing too; the lowerer is what restricts
                    // payloads to variables / wildcards.
                    let (arg_vars, result_var) = self.instantiate_ctor(&scheme)?;
                    self.eq(pat.span, result_var, scrut_var);
                    for (sub, av) in args.iter().zip(arg_vars) {
                        self.constrain_pattern(local, sub, av)?;
                        // Record this sub-pattern's own instantiated field type so
                        // the lowerer can recover a NESTED record / list sub-pattern's
                        // complete shape the same way a top-level `case` / `let` binder
                        // already does (identical precedent in `constrain_lambda`, the
                        // `regions.insert` on every lambda-parameter span below).
                        // Class 4 item C —
                        // docs/adr/0010-pattern-and-lowering-completeness.md.
                        self.regions
                            .insert((self.current_home.clone(), sub.span), av);
                    }
                } else {
                    // A constructor with no registered scheme (imported, outside the
                    // single-module subset): fall back to the bare enum type.
                    // We still must recurse into every argument sub-pattern so that
                    // pattern variables (e.g. `Chunk text` where `Chunk` is an
                    // imported ctor) get bound into `local`.  Without the recursion
                    // the body sees `VarLocal("text")` that is absent from the local
                    // map and fires the "unbound local" ICE.  Use a fresh flex
                    // variable per arg since the field types are unknown.
                    let ctor = self.con_var(home.clone(), *type_name, Vec::new())?;
                    self.eq(pat.span, ctor, scrut_var);
                    for sub in args {
                        let av = self.flex()?;
                        self.constrain_pattern(local, sub, av)?;
                    }
                }
                Ok(())
            }
            canon::Pattern_::PTuple(elems) => {
                // A tuple pattern matches a Tuple type element-wise: mint one
                // fresh variable per element, tie the scrutinee to the product
                // over them, and constrain each sub-pattern against its element's
                // variable. Nested sub-patterns recurse; the lowerer restricts
                // which element shapes it can actually emit.
                let mut elem_vars = Vec::with_capacity(elems.len());
                for _ in elems {
                    elem_vars.push(self.flex()?);
                }
                let tuple = self.structure(FlatType::Tuple(elem_vars.clone()))?;
                self.eq(pat.span, tuple, scrut_var);
                for (sub, ev) in elems.iter().zip(elem_vars) {
                    self.constrain_pattern(local, sub, ev)?;
                    // Same region-threading as the `PCtor` arm above so a record
                    // (or list) nested inside a TUPLE element (`(Ok {name}, y)`)
                    // recovers its complete shape in the lowerer. Class 4 item C.
                    self.regions
                        .insert((self.current_home.clone(), sub.span), ev);
                }
                Ok(())
            }
            canon::Pattern_::PRecord(fields) => {
                // A field-pun record pattern `{ x, y }` binds each named field of
                // the scrutinee record. Closed records carry no row variable, so
                // the scrutinee's full field set may not be settled here; instead
                // of forcing an exact-shape unification (which would reject the
                // legal subset pattern `{ x }` on a `{ x, y }` record), each
                // field is pulled out with the SAME deferred field-access channel
                // a `record.field` expression uses. After the main solve,
                // `resolve_field_accesses` links each binder to the field's type.
                for f in fields {
                    let result = self.flex()?;
                    self.field_accesses.push(FieldAccess {
                        record: scrut_var,
                        field: f.value,
                        result,
                        span: f.span,
                        home: self.current_home.clone(),
                    });
                    local.insert(f.value, result);
                }
                Ok(())
            }
            // A literal pattern pins the scrutinee to the literal's type. It
            // binds no names. A mismatch (`case n of "x" -> …` with `n : Int`)
            // surfaces as the ordinary IPE-T0001 type mismatch.
            canon::Pattern_::PInt(_) => {
                let lit = self.int_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            canon::Pattern_::PBool(_) => {
                let lit = self.bool_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            canon::Pattern_::PChar(_) => {
                let lit = self.char_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            canon::Pattern_::PStr(_) => {
                let lit = self.string_var()?;
                self.eq(pat.span, lit, scrut_var);
                Ok(())
            }
            // An alias `inner as name` binds `name` to the whole scrutinee and
            // additionally constrains the inner pattern against it.
            canon::Pattern_::PAlias(inner, name) => {
                local.insert(name.value, scrut_var);
                self.constrain_pattern(local, inner, scrut_var)
            }
            // A list pattern `[a, b]` matches a `List elem`: each element
            // sub-pattern is constrained against one shared element variable, and
            // the scrutinee is tied to the list over it.
            canon::Pattern_::PList(elems) => {
                let elem = self.flex()?;
                let list = self.list_var(elem)?;
                self.eq(pat.span, list, scrut_var);
                for sub in elems {
                    self.constrain_pattern(local, sub, elem)?;
                }
                Ok(())
            }
            // A cons pattern `head :: tail` matches a `List elem`: `head : elem`,
            // `tail : List elem` (the scrutinee's own type), scrutinee `List elem`.
            canon::Pattern_::PCons(head, tail) => {
                let elem = self.flex()?;
                let list = self.list_var(elem)?;
                self.eq(pat.span, list, scrut_var);
                self.constrain_pattern(local, head, elem)?;
                self.constrain_pattern(local, tail, list)
            }
            // An or-pattern `p1 | p2 | …`: every alternative is constrained
            // against the SAME scrutinee variable, and its binders are unified
            // name-by-name with the first alternative's, so the arm body reads
            // one binder environment. Canon already proved the alternatives bind
            // the identical name set (IPE-T0019); unifying each shared name's
            // var here is the same-type half of the rule — a failure surfaces as
            // the ordinary IPE-T0001 mismatch attributed to the alternative. The
            // body is constrained ONCE afterwards, in `local`, never per
            // alternative.
            canon::Pattern_::POr(alts) => {
                let Some((first, rest)) = alts.split_first() else {
                    return Err(Diagnostic::CompilerBug {
                        where_: STAGE,
                        detail: "an or-pattern reached type inference with no alternatives"
                            .to_owned(),
                    });
                };
                // The first alternative binds directly into the shared `local`.
                self.constrain_pattern(local, first, scrut_var)?;
                for alt in rest {
                    let mut alt_local: BTreeMap<Symbol, VarId> = BTreeMap::new();
                    self.constrain_pattern(&mut alt_local, alt, scrut_var)?;
                    // Unify each of this alternative's binders with the reference
                    // binder of the same name established by the first alternative.
                    for (name, var) in alt_local {
                        if let Some(reference) = local.get(&name).copied() {
                            self.eq(alt.span, reference, var);
                        } else {
                            // Unreachable: canon proved every alternative binds
                            // the same names. Adopt the binder rather than drop it.
                            local.insert(name, var);
                        }
                    }
                }
                Ok(())
            }
        }
    }

    /// Build the IPE-T0013 diagnostic for a constructor pattern that binds the
    /// wrong number of payload fields. A forged constructor symbol surfaces the
    /// underlying intern bug instead.
    pub(crate) fn ctor_pattern_arity(
        &self,
        span: Span,
        ctor: Symbol,
        expected: usize,
        found: usize,
    ) -> Diagnostic {
        self.interner.resolve(ctor).map_or_else(
            || Diagnostic::CompilerBug {
                where_: "intern.resolve",
                detail: format!("no backing string for constructor symbol {}", ctor.as_raw()),
            },
            |s| Diagnostic::Type {
                span,
                msg: TypeError::CtorPatternArity {
                    ctor: Box::from(s),
                    expected,
                    found,
                },
            },
        )
    }
}
