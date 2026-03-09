(function_item
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "FUNCTION"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(function_signature_item
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "FUNCTION"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(struct_item
  name: (type_identifier) @name)
{
  node @name.node
  attr (@name.node) kind = "STRUCT"
  attr (@name.node) canonical_role = "declaration"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

(enum_item
  name: (type_identifier) @name)
{
  node @name.node
  attr (@name.node) kind = "ENUM"
  attr (@name.node) canonical_role = "declaration"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

(union_item
  name: (type_identifier) @name)
{
  node @name.node
  attr (@name.node) kind = "UNION"
  attr (@name.node) canonical_role = "declaration"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

(trait_item
  name: (type_identifier) @name)
{
  node @name.node
  attr (@name.node) kind = "INTERFACE"
  attr (@name.node) canonical_role = "declaration"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

(type_item
  name: (type_identifier) @name)
{
  node @name.node
  attr (@name.node) kind = "TYPEDEF"
  attr (@name.node) canonical_role = "declaration"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

(macro_definition
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "MACRO"
  attr (@name.node) canonical_role = "declaration"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(mod_item
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "MODULE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(field_declaration
  name: (field_identifier) @name)
{
  node @name.node
  attr (@name.node) kind = "FIELD"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

;; Struct field membership
(struct_item
  name: (type_identifier) @struct_name
  (field_declaration_list
    (field_declaration name: (field_identifier) @field_name)))
{
  edge @struct_name.node -> @field_name.node
  attr (@struct_name.node -> @field_name.node) kind = "MEMBER"
}

;; Module membership
(mod_item
  name: (identifier) @mod_name
  body: (declaration_list
    (function_item name: (identifier) @member_name)))
{
  edge @mod_name.node -> @member_name.node
  attr (@mod_name.node -> @member_name.node) kind = "MEMBER"
}

(mod_item
  name: (identifier) @mod_name
  body: (declaration_list
    (struct_item name: (type_identifier) @member_name)))
{
  edge @mod_name.node -> @member_name.node
  attr (@mod_name.node -> @member_name.node) kind = "MEMBER"
}

(mod_item
  name: (identifier) @mod_name
  body: (declaration_list
    (enum_item name: (type_identifier) @member_name)))
{
  edge @mod_name.node -> @member_name.node
  attr (@mod_name.node -> @member_name.node) kind = "MEMBER"
}

(mod_item
  name: (identifier) @mod_name
  body: (declaration_list
    (trait_item name: (type_identifier) @member_name)))
{
  edge @mod_name.node -> @member_name.node
  attr (@mod_name.node -> @member_name.node) kind = "MEMBER"
}

(trait_item
  name: (type_identifier) @trait_name
  body: (declaration_list
    (function_signature_item name: (identifier) @method_name)))
{
  edge @trait_name.node -> @method_name.node
  attr (@trait_name.node -> @method_name.node) kind = "MEMBER"
}

(trait_item
  name: (type_identifier) @trait_name
  body: (declaration_list
    (function_item name: (identifier) @method_name)))
{
  edge @trait_name.node -> @method_name.node
  attr (@trait_name.node -> @method_name.node) kind = "MEMBER"
}

(mod_item
  name: (identifier) @mod_name
  body: (declaration_list
    (mod_item name: (identifier) @member_name)))
{
  edge @mod_name.node -> @member_name.node
  attr (@mod_name.node -> @member_name.node) kind = "MEMBER"
}

;; Impl type node (normalize broad type expressions in Rust post-processing)
(impl_item
  type: (_) @impl_type_expr)
{
  node @impl_type_expr.node
  attr (@impl_type_expr.node) kind = "CLASS"
  attr (@impl_type_expr.node) canonical_role = "impl_anchor"
  attr (@impl_type_expr.node) rust_impl_expr = "type"
  attr (@impl_type_expr.node) name = (source-text @impl_type_expr)
  attr (@impl_type_expr.node) start_row = (start-row @impl_type_expr)
  attr (@impl_type_expr.node) start_col = (start-column @impl_type_expr)
  attr (@impl_type_expr.node) end_row = (end-row @impl_type_expr)
  attr (@impl_type_expr.node) end_col = (end-column @impl_type_expr)
}

;; Impl method membership
(impl_item
  type: (_) @impl_type_expr
  body: (declaration_list
    (function_item name: (identifier) @method_name)))
{
  edge @impl_type_expr.node -> @method_name.node
  attr (@impl_type_expr.node -> @method_name.node) kind = "MEMBER"
}

;; Trait implementation (inheritance)
(impl_item
  trait: (_) @impl_trait_expr
  type: (_) @impl_type_expr)
{
  node @impl_trait_expr.node
  attr (@impl_trait_expr.node) kind = "INTERFACE"
  attr (@impl_trait_expr.node) rust_impl_expr = "trait"
  attr (@impl_trait_expr.node) name = (source-text @impl_trait_expr)
  attr (@impl_trait_expr.node) start_row = (start-row @impl_trait_expr)
  attr (@impl_trait_expr.node) start_col = (start-column @impl_trait_expr)
  attr (@impl_trait_expr.node) end_row = (end-row @impl_trait_expr)
  attr (@impl_trait_expr.node) end_col = (end-column @impl_trait_expr)

  edge @impl_type_expr.node -> @impl_trait_expr.node
  attr (@impl_type_expr.node -> @impl_trait_expr.node) kind = "INHERITANCE"
}

;; Calls (global fallback identifier)
(call_expression
  function: (identifier) @callee_any) @call_any
{
  node @call_any.node
  attr (@call_any.node) kind = "UNKNOWN"
  attr (@call_any.node) name = (source-text @callee_any)
  attr (@call_any.node) start_row = (start-row @callee_any)
  attr (@call_any.node) start_col = (start-column @callee_any)
  attr (@call_any.node) end_row = (end-row @callee_any)
  attr (@call_any.node) end_col = (end-column @callee_any)

  edge @call_any.node -> @call_any.node
  attr (@call_any.node -> @call_any.node) kind = "CALL"
  attr (@call_any.node -> @call_any.node) line = (start-row @call_any)
}

;; Calls (global fallback field expression)
(call_expression
  function: (field_expression
    field: (field_identifier) @callee_any) @call_any)
{
  node @call_any.node
  attr (@call_any.node) kind = "UNKNOWN"
  attr (@call_any.node) name = (source-text @callee_any)
  attr (@call_any.node) start_row = (start-row @callee_any)
  attr (@call_any.node) start_col = (start-column @callee_any)
  attr (@call_any.node) end_row = (end-row @callee_any)
  attr (@call_any.node) end_col = (end-column @callee_any)

  edge @call_any.node -> @call_any.node
  attr (@call_any.node -> @call_any.node) kind = "CALL"
  attr (@call_any.node -> @call_any.node) line = (start-row @call_any)
}

;; Calls (global fallback scoped identifier)
(call_expression
  function: (scoped_identifier
    name: (identifier) @callee_any) @call_any)
{
  node @call_any.node
  attr (@call_any.node) kind = "UNKNOWN"
  attr (@call_any.node) name = (source-text @callee_any)
  attr (@call_any.node) start_row = (start-row @callee_any)
  attr (@call_any.node) start_col = (start-column @callee_any)
  attr (@call_any.node) end_row = (end-row @callee_any)
  attr (@call_any.node) end_col = (end-column @callee_any)

  edge @call_any.node -> @call_any.node
  attr (@call_any.node -> @call_any.node) kind = "CALL"
  attr (@call_any.node -> @call_any.node) line = (start-row @call_any)
}

;; Imports (use declarations)
(use_declaration
  argument: (scoped_identifier) @module)
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

  edge @module.node -> @module.node
  attr (@module.node -> @module.node) kind = "IMPORT"
}

(use_declaration
  argument: (use_wildcard) @module)
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

  edge @module.node -> @module.node
  attr (@module.node -> @module.node) kind = "IMPORT"
}

;; Macro usage
(function_item
  name: (identifier) @caller
  body: (block
    (expression_statement
      (macro_invocation macro: (identifier) @macro) @call)))
{
  node @macro.node
  attr (@macro.node) kind = "MACRO"
  attr (@macro.node) name = (source-text @macro)
  attr (@macro.node) start_row = (start-row @macro)
  attr (@macro.node) start_col = (start-column @macro)
  attr (@macro.node) end_row = (end-row @macro)
  attr (@macro.node) end_col = (end-column @macro)

  edge @caller.node -> @macro.node
  attr (@caller.node -> @macro.node) kind = "CALL"
  attr (@caller.node -> @macro.node) line = (start-row @call)
}

;; Lambda assignment
(let_declaration
  pattern: (identifier) @name
  value: (closure_expression) @def)
{
  node @name.node
  attr (@name.node) kind = "FUNCTION"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

;; Local bindings
(let_declaration
  pattern: (identifier) @name
  value: (_) @value
  (#not-match? @value "^(move\\s+)?\\|"))
{
  node @name.node
  attr (@name.node) kind = "VARIABLE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) value_start_row = (start-row @value)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

;; Imports (aliases and list forms)
(use_declaration
  argument: (identifier) @module)
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

  edge @module.node -> @module.node
  attr (@module.node -> @module.node) kind = "IMPORT"
}

(use_declaration
  argument: (use_as_clause
    path: (_) @module_path
    alias: (identifier) @alias_name))
{
  node @module_path.node
  attr (@module_path.node) kind = "MODULE"
  attr (@module_path.node) name = (source-text @module_path)
  attr (@module_path.node) start_row = (start-row @module_path)
  attr (@module_path.node) start_col = (start-column @module_path)
  attr (@module_path.node) end_row = (end-row @module_path)
  attr (@module_path.node) end_col = (end-column @module_path)

  node @alias_name.node
  attr (@alias_name.node) kind = "MODULE"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @alias_name.node -> @module_path.node
  attr (@alias_name.node -> @module_path.node) kind = "IMPORT"
}

(use_declaration
  argument: (use_list
    (identifier) @module))
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

  edge @module.node -> @module.node
  attr (@module.node -> @module.node) kind = "IMPORT"
}

(use_declaration
  argument: (scoped_use_list
    list: (use_list
      (identifier) @module)))
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

  edge @module.node -> @module.node
  attr (@module.node -> @module.node) kind = "IMPORT"
}

(use_declaration
  argument: (use_list
    (use_as_clause
      path: (_) @module_path
      alias: (identifier) @alias_name)))
{
  node @module_path.node
  attr (@module_path.node) kind = "MODULE"
  attr (@module_path.node) name = (source-text @module_path)
  attr (@module_path.node) start_row = (start-row @module_path)
  attr (@module_path.node) start_col = (start-column @module_path)
  attr (@module_path.node) end_row = (end-row @module_path)
  attr (@module_path.node) end_col = (end-column @module_path)

  node @alias_name.node
  attr (@alias_name.node) kind = "MODULE"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @alias_name.node -> @module_path.node
  attr (@alias_name.node -> @module_path.node) kind = "IMPORT"
}

(use_declaration
  argument: (scoped_use_list
    list: (use_list
      (use_as_clause
        path: (_) @module_path
        alias: (identifier) @alias_name))))
{
  node @module_path.node
  attr (@module_path.node) kind = "MODULE"
  attr (@module_path.node) name = (source-text @module_path)
  attr (@module_path.node) start_row = (start-row @module_path)
  attr (@module_path.node) start_col = (start-column @module_path)
  attr (@module_path.node) end_row = (end-row @module_path)
  attr (@module_path.node) end_col = (end-column @module_path)

  node @alias_name.node
  attr (@alias_name.node) kind = "MODULE"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @alias_name.node -> @module_path.node
  attr (@alias_name.node -> @module_path.node) kind = "IMPORT"
}

