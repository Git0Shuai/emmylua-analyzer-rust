use emmylua_parser::{
    LuaAst, LuaAstNode, LuaAstToken, LuaCallExpr, LuaClosureExpr, LuaDocTagCast, LuaExpr,
    LuaFuncStat, LuaIndexExpr, LuaIndexKey, LuaLiteralExpr, LuaLiteralToken, LuaNameExpr,
    LuaTableExpr, LuaVarExpr,
};

use crate::{
    compilation::analyzer::{common::bind_type, unresolve::UnResolveTableField}, db_index::{LuaDecl, LuaMember, LuaMemberKey, LuaMemberOwner}, FileId, InFiled, InferFailReason, LocalAttribute, LuaDeclExtra, LuaDeclId, LuaMemberFeature, LuaMemberId, LuaSignatureId, LuaType, LuaTypeAttribute, LuaTypeCache, LuaTypeDecl, LuaTypeDeclId
};

use super::DeclAnalyzer;

pub fn analyze_name_expr(analyzer: &mut DeclAnalyzer, expr: LuaNameExpr) -> Option<()> {
    let name_token = expr.get_name_token()?;
    let name = name_token.get_name_text();
    let position = name_token.get_position();
    let range = name_token.get_range();
    let file_id = analyzer.get_file_id();
    let decl_id = LuaDeclId::new(file_id, position);
    let (decl_id, is_local) = if let Some(decl) = analyzer.decl.get_decl(&decl_id) {
        if decl.is_local() {
            // 由于 kg_require 的影响被视作 local 声明的赋值语句
            (None, true)
        } else {
            (Some(decl_id), false)
        }
    } else if let Some(decl) = analyzer.find_decl(&name, position) {
        if decl.is_local() {
            // reference local variable
            (Some(decl.get_id()), true)
        } else {
            if decl.get_position() == position {
                return Some(());
            }
            // reference in filed global variable
            (Some(decl.get_id()), false)
        }
    } else {
        (None, false)
    };

    let reference_index = analyzer.db.get_reference_index_mut();

    if let Some(id) = decl_id {
        reference_index.add_decl_reference(id, file_id, range, false);
    }

    if !is_local {
        reference_index.add_global_reference(name, file_id, expr.get_syntax_id());
    }

    Some(())
}

pub fn analyze_index_expr(analyzer: &mut DeclAnalyzer, index_expr: LuaIndexExpr) -> Option<()> {
    if index_expr.ancestors::<LuaDocTagCast>().next().is_some() {
        return Some(());
    }
    let index_key = index_expr.get_index_key()?;
    let key = match index_key {
        LuaIndexKey::Name(name) => LuaMemberKey::Name(name.get_name_text().to_string().into()),
        LuaIndexKey::Integer(int) => {
            if int.is_int() {
                LuaMemberKey::Integer(int.get_int_value())
            } else {
                return None;
            }
        }
        LuaIndexKey::String(string) => LuaMemberKey::Name(string.get_value().into()),
        LuaIndexKey::Expr(_) => return None,
        LuaIndexKey::Idx(i) => LuaMemberKey::Integer(i as i64),
    };

    let file_id = analyzer.get_file_id();
    let syntax_id = index_expr.get_syntax_id();
    let prefix = index_expr.get_prefix_expr()?;
    if let LuaExpr::NameExpr(name_expr) = prefix {
        let name_token = name_expr.get_name_token()?;
        let name_token_text = name_token.get_name_text();
        if name_token_text == "_G" || name_token_text == "_ENV" {
            if let LuaMemberKey::Name(name) = &key {
                analyzer
                    .db
                    .get_reference_index_mut()
                    .add_global_reference(name, file_id, syntax_id);
            }

            return Some(());
        }
    }

    analyzer
        .db
        .get_reference_index_mut()
        .add_index_reference(key, file_id, syntax_id);

    Some(())
}

pub fn analyze_closure_expr(analyzer: &mut DeclAnalyzer, expr: LuaClosureExpr) -> Option<()> {
    let params = expr.get_params_list()?;
    let signature_id = LuaSignatureId::from_closure(analyzer.get_file_id(), &expr);
    let file_id = analyzer.get_file_id();
    let member_id = get_closure_member_id(&expr, file_id);
    try_add_self_param(analyzer, &expr);

    for (idx, param) in params.get_params().enumerate() {
        let name = param.get_name_token().map_or_else(
            || {
                if param.is_dots() {
                    "...".to_string()
                } else {
                    "".to_string()
                }
            },
            |name_token| name_token.get_name_text().to_string(),
        );
        let range = param.get_range();

        let decl = LuaDecl::new(
            &name,
            file_id,
            range,
            LuaDeclExtra::Param {
                idx,
                signature_id,
                owner_member_id: member_id,
            },
            None,
        );

        analyzer.add_decl(decl);
    }

    analyze_closure_params(analyzer, &signature_id, &expr);

    Some(())
}

fn try_add_self_param(analyzer: &mut DeclAnalyzer, closure: &LuaClosureExpr) -> Option<()> {
    let func_stat = closure.get_parent::<LuaFuncStat>()?;
    let func_name = func_stat.get_func_name()?;
    let LuaVarExpr::IndexExpr(index_expr) = func_name else {
        return Some(());
    };

    let index_token = index_expr.get_index_token()?;
    if !index_token.is_colon() {
        return Some(());
    }

    let self_param = LuaDecl::new(
        "self",
        analyzer.get_file_id(),
        index_token.get_range(),
        LuaDeclExtra::ImplicitSelf {
            kind: index_token.syntax().kind(),
        },
        None,
    );

    analyzer.add_decl(self_param);
    Some(())
}

fn get_closure_member_id(closure: &LuaClosureExpr, file_id: FileId) -> Option<LuaMemberId> {
    let parent = closure.get_parent::<LuaAst>()?;
    match parent {
        LuaAst::LuaAssignStat(assign) => {
            let (vars, value_exprs) = assign.get_var_and_expr_list();
            let value_idx = value_exprs
                .iter()
                .position(|expr| expr.get_position() == closure.get_position())?;
            let var = vars.get(value_idx)?;
            if let LuaVarExpr::IndexExpr(index_expr) = var {
                return Some(LuaMemberId::new(index_expr.get_syntax_id(), file_id));
            }
        }
        LuaAst::LuaFuncStat(func_stat) => {
            let func_name = func_stat.get_func_name()?;
            if let LuaVarExpr::IndexExpr(index_expr) = func_name {
                return Some(LuaMemberId::new(index_expr.get_syntax_id(), file_id));
            }
        }
        LuaAst::LuaTableField(table_field) => {
            return Some(LuaMemberId::new(table_field.get_syntax_id(), file_id));
        }
        _ => {}
    }

    None
}

fn analyze_closure_params(
    analyzer: &mut DeclAnalyzer,
    signature_id: &LuaSignatureId,
    closure: &LuaClosureExpr,
) -> Option<()> {
    let signature = analyzer
        .db
        .get_signature_index_mut()
        .get_or_create(signature_id.clone());
    let params = closure.get_params_list()?.get_params();
    for param in params {
        let name = if let Some(name_token) = param.get_name_token() {
            name_token.get_name_text().to_string()
        } else if param.is_dots() {
            "...".to_string()
        } else {
            return None;
        };

        signature.params.push(name);
    }

    Some(())
}

pub fn analyze_table_expr(analyzer: &mut DeclAnalyzer, table_expr: LuaTableExpr) -> Option<()> {
    if table_expr.is_object() {
        let file_id = analyzer.get_file_id();
        let owner_id = LuaMemberOwner::Element(InFiled {
            file_id,
            value: table_expr.get_range(),
        });
        let decl_feature = if analyzer.is_meta {
            LuaMemberFeature::MetaDefine
        } else {
            LuaMemberFeature::FileDefine
        };

        for field in table_expr.get_fields() {
            if let Some(field_key) = field.get_field_key() {
                let key: LuaMemberKey = match field_key {
                    LuaIndexKey::Name(name) => LuaMemberKey::Name(name.get_name_text().into()),
                    LuaIndexKey::String(str) => LuaMemberKey::Name(str.get_value().into()),
                    LuaIndexKey::Integer(i) => LuaMemberKey::Integer(i.get_int_value()),
                    LuaIndexKey::Idx(idx) => LuaMemberKey::Integer(idx as i64),
                    LuaIndexKey::Expr(field_expr) => {
                        let unresolve_member = UnResolveTableField {
                            file_id: analyzer.get_file_id(),
                            table_expr: table_expr.clone(),
                            field: field.clone(),
                            decl_feature,
                        };
                        analyzer.context.add_unresolve(
                            unresolve_member.into(),
                            InferFailReason::UnResolveExpr(InFiled::new(
                                analyzer.get_file_id(),
                                field_expr.clone(),
                            )),
                        );
                        continue;
                    }
                };

                analyzer.db.get_reference_index_mut().add_index_reference(
                    key.clone(),
                    file_id,
                    field.get_syntax_id(),
                );

                let member_id = LuaMemberId::new(field.get_syntax_id(), file_id);
                let member = match &owner_id {
                    LuaMemberOwner::GlobalPath(path) => {
                        LuaMember::new(member_id, key, decl_feature, Some(path.clone()))
                    }
                    _ => LuaMember::new(member_id, key, decl_feature, None),
                };
                analyzer
                    .db
                    .get_member_index_mut()
                    .add_member(owner_id.clone(), member);
            }
        }
    }

    Some(())
}

pub fn analyze_literal_expr(analyzer: &mut DeclAnalyzer, expr: LuaLiteralExpr) -> Option<()> {
    let literal = expr.get_literal()?;
    let file_id = analyzer.get_file_id();

    match literal {
        LuaLiteralToken::String(string_token) => {
            if !analyzer.db.get_emmyrc().references.short_string_search {
                return Some(());
            }

            let value = string_token.get_value();
            if value.len() <= 64 {
                analyzer.db.get_reference_index_mut().add_string_reference(
                    file_id,
                    &value,
                    string_token.get_range(),
                );
            }
        }
        LuaLiteralToken::Dots(dots_token) => {
            let position = dots_token.get_position();
            let range = dots_token.get_range();

            let decl_id = LuaDeclId::new(file_id, position);
            let decl_id = analyzer
                .decl
                .get_decl(&decl_id)
                .map(|_| decl_id)
                .or_else(|| {
                    analyzer
                        .find_decl(&dots_token.get_text(), position)
                        .and_then(|decl| decl.is_local().then(|| decl.get_id()))
                });

            if let Some(id) = decl_id {
                analyzer
                    .db
                    .get_reference_index_mut()
                    .add_decl_reference(id, file_id, range, false);
            }
        }
        _ => {}
    }

    Some(())
}

pub fn analyze_call_expr(analyzer: &mut DeclAnalyzer, expr: LuaCallExpr) -> Option<()> {
    if expr.is_require() || expr.is_kg_require() {
        let args = expr.get_args_list()?;
        if let Some(LuaExpr::LiteralExpr(literal_expr)) = args.get_args().next() {
            if let Some(LuaLiteralToken::String(string_token)) = literal_expr.get_literal() {
                let module_path = string_token.get_value();
                let file_id = analyzer.get_file_id();
                let module_info = analyzer.db.get_module_index().find_module(&module_path)?;
                let module_file_id = module_info.file_id;
                analyzer
                    .db
                    .get_file_dependencies_index_mut()
                    .add_required_file(file_id, module_file_id);
            }
        }
    } else if expr.is_define_class() || expr.is_define_entity() {
        let args = expr.get_args_list()?;
        let mut args_iter = args.get_args();
        if let Some(LuaExpr::LiteralExpr(literal_expr)) = args_iter.next() {
            if let Some(LuaLiteralToken::String(string_token)) = literal_expr.get_literal() {
                let class_name = string_token.get_value();
                let file_id = analyzer.get_file_id();
                let lua_decl = LuaDecl::new(
                    &class_name,
                    file_id,
                    string_token.get_range(),
                    LuaDeclExtra::Local {
                        kind: string_token.syntax().kind(),
                        attrib: Some(LocalAttribute::Module)
                    },
                    Some(expr.get_syntax_id()),
                );
                let lua_decl_id = lua_decl.get_id();
                analyzer.add_decl(lua_decl);

                let type_decl = LuaTypeDecl::new(
                    file_id,
                    string_token.get_range(),
                    class_name.to_string(),
                    crate::LuaDeclTypeKind::Class,
                    LuaTypeAttribute::None.into(),
                    LuaTypeDeclId::new(&class_name),
                );
                let type_decl_id = type_decl.get_id();
                analyzer
                    .db
                    .get_type_index_mut()
                    .add_type_decl(file_id, type_decl);

                bind_type(
                    analyzer.db,
                    lua_decl_id.into(),
                    LuaTypeCache::DocType(LuaType::Def(type_decl_id.clone())),
                );
            }
        }
    }

    Some(())
}
