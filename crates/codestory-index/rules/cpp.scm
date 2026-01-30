(namespace_definition
  name: (_) @name)
{
  node @name.node
  attr (@name.node) kind = "MODULE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

(class_specifier
  name: (_) @name)
{
  node @name.node
  attr (@name.node) kind = "CLASS"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

(function_definition
  declarator: (function_declarator
    declarator: (_) @name)) @def
{
  node @name.node
  attr (@name.node) kind = "FUNCTION"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(field_declaration
  declarator: (field_identifier) @name)
{
  node @name.node
  attr (@name.node) kind = "VARIABLE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

;; Namespace membership
(namespace_definition
  name: (_) @ns_name
  body: (declaration_list
    (class_specifier name: (_) @member_name)))
{
  edge @ns_name.node -> @member_name.node
  attr (@ns_name.node -> @member_name.node) kind = "MEMBER"
}

(namespace_definition
  name: (_) @ns_name
  body: (declaration_list
    (function_definition
      declarator: (function_declarator
        declarator: (_) @member_name))))
{
  edge @ns_name.node -> @member_name.node
  attr (@ns_name.node -> @member_name.node) kind = "MEMBER"
}

;; Class members (methods)
(class_specifier
  name: (_) @class_name
  body: (field_declaration_list
    (function_definition
      declarator: (function_declarator
        declarator: (_) @method_name))))
{
  edge @class_name.node -> @method_name.node
  attr (@class_name.node -> @method_name.node) kind = "MEMBER"
}

;; Class members (fields)
(class_specifier
  name: (_) @class_name
  body: (field_declaration_list
    (field_declaration declarator: (field_identifier) @field_name)))
{
  edge @class_name.node -> @field_name.node
  attr (@class_name.node -> @field_name.node) kind = "MEMBER"
}

;; Calls (identifier)
(function_definition
  declarator: (function_declarator
    declarator: (_) @caller)
  body: (compound_statement
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
(function_definition
  declarator: (function_declarator
    declarator: (_) @caller)
  body: (compound_statement
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

;; Includes
(preproc_include
  path: (system_lib_string) @module)
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

(preproc_include
  path: (string_literal) @module)
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
