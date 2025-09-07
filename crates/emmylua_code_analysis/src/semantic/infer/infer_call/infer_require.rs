use emmylua_parser::LuaCallExpr;

use crate::{
    DbIndex, InFiled, InferFailReason, LuaInferCache, LuaType, infer_expr,
    semantic::infer::InferResult,
};

pub fn infer_require_call(
    db: &DbIndex,
    cache: &mut LuaInferCache,
    call_expr: LuaCallExpr,
) -> InferResult {
    let arg_list = call_expr.get_args_list().ok_or(InferFailReason::None)?;
    let first_arg = arg_list.get_args().next().ok_or(InferFailReason::None)?;
    let require_path_type = infer_expr(db, cache, first_arg)?;
    let module_path: String = match &require_path_type {
        LuaType::StringConst(module_path) => module_path.as_ref().to_string(),
        _ => {
            return Ok(LuaType::Any);
        }
    };

    let module_info = db
        .get_module_index()
        .find_module(&module_path)
        .ok_or(InferFailReason::None)?;
    match &module_info.export_type {
        Some(ty) => match ty {
            LuaType::Def(id) => Ok(LuaType::Ref(id.clone())),
            _ => Ok(ty.clone()),
        },
        None => Err(InferFailReason::UnResolveExpr(InFiled::new(
            cache.get_file_id(),
            call_expr.into(),
        ))),
    }
}

pub fn infer_kg_require_call(
    db: &DbIndex,
    cache: &mut LuaInferCache,
    call_expr: LuaCallExpr,
) -> InferResult {
    let arg_list = call_expr.get_args_list().ok_or(InferFailReason::None)?;
    let first_arg = arg_list.get_args().next().ok_or(InferFailReason::None)?;
    let require_path_type = infer_expr(db, cache, first_arg)?;
    let module_path: String = match &require_path_type {
        LuaType::StringConst(module_path) => module_path.as_ref().to_string(),
        _ => {
            return Ok(LuaType::Any);
        }
    };

    let module_info = db
        .get_module_index()
        .find_module(&module_path)
        .ok_or(InferFailReason::None)?;
    match &module_info.export_type {
        Some(ty) => match ty {
            LuaType::Def(id) => Ok(LuaType::Ref(id.clone())),
            _ => Ok(ty.clone()),
        },
        None => Ok(LuaType::FileEnv(module_info.file_id))
    }
}
