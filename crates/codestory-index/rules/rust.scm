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

(struct_item
  name: (type_identifier) @name)
{
  node @name.node
  attr (@name.node) kind = "CLASS"
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
  attr (@name.node) kind = "CLASS"
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
  attr (@name.node) kind = "CLASS"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
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

(mod_item
  name: (identifier) @mod_name
  body: (declaration_list
    (mod_item name: (identifier) @member_name)))
{
  edge @mod_name.node -> @member_name.node
  attr (@mod_name.node -> @member_name.node) kind = "MEMBER"
}

;; Impl type node (ensure single capture per impl_item)
(impl_item
  type: (type_identifier) @type_name)
{
  node @type_name.node
  attr (@type_name.node) kind = "CLASS"
  attr (@type_name.node) name = (source-text @type_name)
  attr (@type_name.node) start_row = (start-row @type_name)
  attr (@type_name.node) start_col = (start-column @type_name)
  attr (@type_name.node) end_row = (end-row @type_name)
  attr (@type_name.node) end_col = (end-column @type_name)
}

;; Impl method membership
(impl_item
  type: (type_identifier) @type_name
  body: (declaration_list
    (function_item name: (identifier) @method_name)))
{
  edge @type_name.node -> @method_name.node
  attr (@type_name.node -> @method_name.node) kind = "MEMBER"
}

;; Trait implementation (inheritance)
(impl_item
  trait: (type_identifier) @trait_name
  type: (type_identifier) @type_name)
{
  node @trait_name.node
  attr (@trait_name.node) kind = "CLASS"
  attr (@trait_name.node) name = (source-text @trait_name)
  attr (@trait_name.node) start_row = (start-row @trait_name)
  attr (@trait_name.node) start_col = (start-column @trait_name)
  attr (@trait_name.node) end_row = (end-row @trait_name)
  attr (@trait_name.node) end_col = (end-column @trait_name)

  edge @type_name.node -> @trait_name.node
  attr (@type_name.node -> @trait_name.node) kind = "INHERITANCE"
}

;; Calls (identifier)
(function_item
  name: (identifier) @caller
  body: (block
    (expression_statement
      (call_expression function: (identifier) @callee) @call)))
{
  node @callee.node
  attr (@callee.node) kind = "UNKNOWN"
  attr (@callee.node) name = (source-text @callee)
  attr (@callee.node) start_row = (start-row @callee)
  attr (@callee.node) start_col = (start-column @callee)
  attr (@callee.node) end_row = (end-row @callee)
  attr (@callee.node) end_col = (end-column @callee)

  edge @caller.node -> @callee.node
  attr (@caller.node -> @callee.node) kind = "CALL"
  attr (@caller.node -> @callee.node) line = (start-row @call)
}

;; Calls (field expression)
(function_item
  name: (identifier) @caller
  body: (block
    (expression_statement
      (call_expression function: (field_expression field: (field_identifier) @callee) @call))))
{
  node @callee.node
  attr (@callee.node) kind = "UNKNOWN"
  attr (@callee.node) name = (source-text @callee)
  attr (@callee.node) start_row = (start-row @callee)
  attr (@callee.node) start_col = (start-column @callee)
  attr (@callee.node) end_row = (end-row @callee)
  attr (@callee.node) end_col = (end-column @callee)

  edge @caller.node -> @callee.node
  attr (@caller.node -> @callee.node) kind = "CALL"
  attr (@caller.node -> @callee.node) line = (start-row @call)
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

;; Macro usage
(function_item
  name: (identifier) @caller
  body: (block
    (expression_statement
      (macro_invocation macro: (identifier) @macro) @call)))
{
  node @macro.node
  attr (@macro.node) kind = "UNKNOWN"
  attr (@macro.node) name = (source-text @macro)
  attr (@macro.node) start_row = (start-row @macro)
  attr (@macro.node) start_col = (start-column @macro)
  attr (@macro.node) end_row = (end-row @macro)
  attr (@macro.node) end_col = (end-column @macro)

  edge @caller.node -> @macro.node
  attr (@caller.node -> @macro.node) kind = "USAGE"
  attr (@caller.node -> @macro.node) line = (start-row @call)
}
