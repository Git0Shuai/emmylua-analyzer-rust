use std::path::PathBuf;

use emmylua_code_analysis::{DbIndex, Emmyrc, LuaDocument, file_path_to_uri};
use emmylua_parser::{
    LuaAstNode, LuaAstToken, LuaCallArgList, LuaCallExpr, LuaLiteralExpr, LuaStringToken,
    LuaSyntaxNode,
};
use lsp_types::DocumentLink;

pub fn build_links(
    db: &DbIndex,
    root: LuaSyntaxNode,
    document: &LuaDocument,
    emmyrc: &Emmyrc,
) -> Option<Vec<DocumentLink>> {
    let string_tokens = root
        .descendants_with_tokens()
        .filter_map(|it| it.into_token())
        .filter_map(|it| LuaStringToken::cast(it));

    let mut result = vec![];
    for token in string_tokens {
        try_build_file_link(db, token, document, &mut result, &emmyrc);
    }

    Some(result)
}

fn try_build_file_link(
    db: &DbIndex,
    token: LuaStringToken,
    document: &LuaDocument,
    result: &mut Vec<DocumentLink>,
    emmyrc: &Emmyrc,
) -> Option<()> {
    if is_require_path(token.clone()).unwrap_or(false) {
        try_build_module_link(db, token, document, result);
        return Some(());
    }

    let file_path = token.get_value();
    if file_path.find(|c| c == '\\' || c == '/').is_some() {
        let suffix_path = PathBuf::from(file_path);
        if suffix_path.exists() {
            if let Some(uri) = file_path_to_uri(&suffix_path) {
                let document_link = DocumentLink {
                    target: Some(uri),
                    range: document.to_lsp_range(token.get_range())?,
                    tooltip: None,
                    data: None,
                };

                result.push(document_link);
            }
            return Some(());
        }

        let resource_paths = emmyrc.resource.paths.clone();
        for resource_path in resource_paths {
            let full_path = PathBuf::from(resource_path).join(&suffix_path);
            if full_path.exists() {
                if let Some(uri) = file_path_to_uri(&full_path) {
                    let document_link = DocumentLink {
                        target: Some(uri),
                        range: document.to_lsp_range(token.get_range())?,
                        tooltip: None,
                        data: None,
                    };

                    result.push(document_link);
                }
                return Some(());
            }
        }
    }

    Some(())
}

fn try_build_module_link(
    db: &DbIndex,
    token: LuaStringToken,
    document: &LuaDocument,
    result: &mut Vec<DocumentLink>,
) -> Option<()> {
    let module_path = token.get_value();
    let module_index = db.get_module_index();
    let founded_module = module_index.find_module(&module_path)?;
    let file_id = founded_module.file_id;
    let vfs = db.get_vfs();
    let uri = vfs.get_uri(&file_id)?;
    let range = token.get_range();
    let lsp_range = document.to_lsp_range(range)?;
    let document_link = DocumentLink {
        target: Some(uri.clone()),
        range: lsp_range,
        tooltip: None,
        data: None,
    };

    result.push(document_link);

    Some(())
}

pub fn is_require_path(token: LuaStringToken) -> Option<bool> {
    let call_expr = token
        .get_parent::<LuaLiteralExpr>()?
        .get_parent::<LuaCallArgList>()?
        .get_parent::<LuaCallExpr>()?;

    Some(call_expr.is_require() || call_expr.is_kg_require())
}
