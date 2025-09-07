use emmylua_parser::{LuaAstNode, LuaChunk, LuaExpr};

use crate::{
    InferFailReason, LuaDeclId, LuaSemanticDeclId, LuaSignatureId,
    compilation::analyzer::unresolve::UnResolveModule, db_index::LuaType,
};

use super::{LuaAnalyzer, LuaReturnPoint, func_body::analyze_func_body_returns};

pub fn analyze_chunk_return(analyzer: &mut LuaAnalyzer, chunk: LuaChunk) -> Option<()> {
    let block = chunk.get_block()?;
    let return_exprs = analyze_func_body_returns(block);
    if return_exprs.is_empty() {
        let module_info = analyzer
            .db
            .get_module_index_mut()
            .get_module_mut(analyzer.file_id)?;
        if module_info.is_kg_required {
            module_info.export_type = Some(LuaType::FileEnv(analyzer.file_id));
        }
    } else{
        for point in return_exprs {
            match point {
                LuaReturnPoint::Expr(expr) => {
                    let expr_type = match analyzer.infer_expr(&expr) {
                        Ok(expr_type) => expr_type,
                        Err(InferFailReason::None) => LuaType::Unknown,
                        Err(reason) => {
                            let unresolve = UnResolveModule {
                                file_id: analyzer.file_id,
                                expr,
                            };
                            analyzer.context.add_unresolve(unresolve.into(), reason);
                            return None;
                        }
                    };

                    let semantic_id = get_semantic_id(analyzer, expr.clone());

                    let module_info = analyzer
                        .db
                        .get_module_index_mut()
                        .get_module_mut(analyzer.file_id)?;
                    match expr_type {
                        LuaType::Variadic(multi) => {
                            let ty = multi.get_type(0)?;
                            module_info.export_type = Some(ty.clone());
                        }
                        _ => {
                            module_info.export_type = Some(expr_type);
                        }
                    }
                    module_info.semantic_id = semantic_id;
                    break;
                }
                // Other cases are stupid code
                _ => {}
            }
        }
    }

    Some(())
}

fn get_semantic_id(analyzer: &LuaAnalyzer, expr: LuaExpr) -> Option<LuaSemanticDeclId> {
    match expr {
        LuaExpr::NameExpr(name_expr) => {
            let name = name_expr.get_name_text()?;
            let tree = analyzer
                .db
                .get_decl_index()
                .get_decl_tree(&analyzer.file_id)?;
            let decl = tree.find_local_decl(&name, name_expr.get_position())?;

            Some(LuaSemanticDeclId::LuaDecl(decl.get_id()))
        }
        LuaExpr::ClosureExpr(closure) => Some(LuaSemanticDeclId::Signature(
            LuaSignatureId::from_closure(analyzer.file_id, &closure),
        )),
        // `return {}`
        LuaExpr::TableExpr(table_expr) => Some(LuaSemanticDeclId::LuaDecl(LuaDeclId::new(
            analyzer.file_id,
            table_expr.get_position(),
        ))),
        _ => None,
    }
}
