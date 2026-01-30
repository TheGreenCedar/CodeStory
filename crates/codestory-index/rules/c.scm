(function_definition
  declarator: (function_declarator
    declarator: (identifier) @name)) @def
{
  node @name.node
  attr (@name.node) kind = "FUNCTION"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

;; Calls
(function_definition
  declarator: (function_declarator
    declarator: (identifier) @caller)
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
