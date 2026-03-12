pub(crate) use svelte_syntax::{
    RawField, attach_estree_comments_to_tree, estree_node_field, estree_node_field_array,
    estree_node_field_object, estree_node_field_str, estree_node_type, estree_value_to_usize,
    expression_identifier_name, expression_literal_string, modern_node_end, modern_node_span,
    modern_node_start, normalize_estree_node, parse_all_comment_nodes, parse_leading_comment_nodes,
    position_raw_node, walk_estree_node,
};
