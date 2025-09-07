mod diagnostic_tags;
mod field_or_operator_def_tags;
mod file_generic_index;
mod infer_type;
mod property_tags;
mod tags;
mod type_def_tags;
mod type_ref_tags;

use super::AnalyzeContext;
use crate::{
    FileId, LuaSemanticDeclId, LuaType,
    compilation::analyzer::AnalysisPipeline,
    db_index::{DbIndex, LuaTypeDeclId},
    profile::Profile,
};
use emmylua_parser::{
    LuaAstNode, LuaCallExpr, LuaComment, LuaExpr, LuaLiteralToken, LuaSyntaxNode,
};
use file_generic_index::FileGenericIndex;
use tags::get_owner_id;

pub struct DocAnalysisPipeline;

impl AnalysisPipeline for DocAnalysisPipeline {
    fn analyze(db: &mut DbIndex, context: &mut AnalyzeContext) {
        let _p = Profile::cond_new("doc analyze", context.tree_list.len() > 1);
        let tree_list = context.tree_list.clone();
        for in_filed_tree in tree_list.iter() {
            let root = &in_filed_tree.value;
            let mut generic_index = FileGenericIndex::new();
            for node in root.syntax().descendants() {
                if let Some(comment) = LuaComment::cast(node.clone()) {
                    let mut analyzer = DocAnalyzer::new(
                        db,
                        in_filed_tree.file_id,
                        &mut generic_index,
                        comment,
                        root.syntax().clone(),
                        context,
                    );
                    analyze_comment(&mut analyzer);
                } else if let Some(call_expr) = LuaCallExpr::cast(node) {
                    if call_expr.is_define_class() || call_expr.is_define_entity() {
                        analyze_call_define_class_expr(db, in_filed_tree.file_id, call_expr);
                    }
                }
            }
        }
    }
}

fn analyze_comment(analyzer: &mut DocAnalyzer) -> Option<()> {
    let comment = analyzer.comment.clone();
    for tag in comment.get_doc_tags() {
        tags::analyze_tag(analyzer, tag);
    }

    let owner = get_owner_id(analyzer)?;
    let comment_description = preprocess_description(
        &comment.get_description()?.get_description_text(),
        Some(&owner),
    );
    analyzer.db.get_property_index_mut().add_description(
        analyzer.file_id,
        owner,
        comment_description,
    );

    Some(())
}

fn analyze_call_define_class_expr(db: &mut DbIndex, file_id: FileId, expr: LuaCallExpr) {
    let Some(args) = expr.get_args_list() else {
        return;
    };
    let mut args_iter = args.get_args();
    let Some(LuaExpr::LiteralExpr(literal_expr)) = args_iter.next() else {
        return;
    };
    let Some(LuaLiteralToken::String(string_token)) = literal_expr.get_literal() else {
        return;
    };

    let class_name = string_token.get_value();
    let mut add_super = |expr| {
        match expr {
            LuaExpr::NameExpr(lua_name_expr) => lua_name_expr.get_name_text(),
            LuaExpr::IndexExpr(lua_index_expr) => lua_index_expr
                .get_index_name_token()
                .map(|it| it.text().to_owned()),
            _ => None,
        }
        .map(|it| {
            let super_type_decl_id = LuaTypeDeclId::new(&it);
            if db
                .get_type_index()
                .get_type_decl(&super_type_decl_id)
                .is_some()
            {
                db.get_type_index_mut().add_super_type(
                    LuaTypeDeclId::new(&class_name),
                    file_id,
                    LuaType::Ref(super_type_decl_id),
                );
            }
        });
    };
    if expr.is_define_class() {
        while let Some(expr) = args_iter.next() {
            add_super(expr);
        }
    } else if expr.is_define_entity() {
        let Some(expr) = args_iter.next() else {
            return;
        };
        if let LuaExpr::TableExpr(super_table_expr) = expr {
            for super_expr in super_table_expr
                .get_fields()
                .flat_map(|it| it.get_value_expr())
            {
                add_super(super_expr)
            }
        }
        let Some(expr) = args_iter.next() else {
            return;
        };
        if let LuaExpr::TableExpr(components_table_expr) = expr {
            for component_expr in components_table_expr
                .get_fields()
                .flat_map(|it| it.get_value_expr())
            {
                match component_expr {
                    LuaExpr::NameExpr(lua_name_expr) => lua_name_expr.get_name_text(),
                    LuaExpr::IndexExpr(lua_index_expr) => lua_index_expr
                        .get_index_name_token()
                        .map(|it| it.text().to_owned()),
                    _ => None,
                }
                .map(|it| {
                    let component_type_decl_id = LuaTypeDeclId::new(&it);
                    if db
                        .get_type_index()
                        .get_type_decl(&component_type_decl_id)
                        .is_some()
                    {
                        db.get_type_index_mut().add_component_type(
                            LuaTypeDeclId::new(&class_name),
                            component_type_decl_id,
                        );
                    }
                });
            }
        }
    }
}

#[derive(Debug)]
pub struct DocAnalyzer<'a> {
    file_id: FileId,
    db: &'a mut DbIndex,
    generic_index: &'a mut FileGenericIndex,
    current_type_id: Option<LuaTypeDeclId>,
    comment: LuaComment,
    root: LuaSyntaxNode,
    is_meta: bool,
    context: &'a mut AnalyzeContext,
}

impl<'a> DocAnalyzer<'a> {
    pub fn new(
        db: &'a mut DbIndex,
        file_id: FileId,
        generic_index: &'a mut FileGenericIndex,
        comment: LuaComment,
        root: LuaSyntaxNode,
        context: &'a mut AnalyzeContext,
    ) -> DocAnalyzer<'a> {
        let is_meta = db.get_module_index().is_meta_file(&file_id);
        DocAnalyzer {
            file_id,
            db,
            generic_index,
            current_type_id: None,
            comment,
            root,
            is_meta,
            context,
        }
    }
}

pub fn preprocess_description(mut description: &str, owner: Option<&LuaSemanticDeclId>) -> String {
    let need_remove_start_char = if let Some(owner) = owner {
        !matches!(owner, LuaSemanticDeclId::Signature(_))
    } else {
        true
    };
    if need_remove_start_char {
        if description.starts_with(['#', '@']) {
            description = description.trim_start_matches(|c| c == '#' || c == '@');
        }
    }

    let mut result = String::new();
    let lines = description.lines();
    let mut in_code_block = false;
    let mut indent = 0;
    for line in lines {
        let trimmed_line = line.trim_start();
        if trimmed_line.starts_with("```") {
            in_code_block = !in_code_block;
            result.push_str(trimmed_line);
            result.push('\n');
            if in_code_block {
                indent = trimmed_line.len() - trimmed_line.trim_start().len();
            }
            continue;
        }

        if in_code_block {
            if indent > 0 && line.len() >= indent {
                let actual_indent = line
                    .chars()
                    .take(indent)
                    .filter(|c| c.is_whitespace())
                    .count();
                result.push_str(&line[actual_indent..]);
            } else {
                result.push_str(line);
            }
        } else {
            result.push_str(trimmed_line);
        }
        result.push('\n');
    }

    // trim end
    result.trim_end().to_string()
}
