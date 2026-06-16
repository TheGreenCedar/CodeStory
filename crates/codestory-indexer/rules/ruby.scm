(method
  name: (_) @name) @def
{
  node @name.node
  attr (@name.node) kind = "METHOD"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(singleton_method
  name: (_) @name) @def
{
  node @name.node
  attr (@name.node) kind = "METHOD"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(class
  name: [
    (constant) @name
    (scope_resolution name: (_) @name)
  ]) @def
{
  node @name.node
  attr (@name.node) kind = "CLASS"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(module
  name: [
    (constant) @name
    (scope_resolution name: (_) @name)
  ]) @def
{
  node @name.node
  attr (@name.node) kind = "MODULE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

;; Inheritance
(class
  name: (constant) @class_name
  (superclass (constant) @parent_name))
{
  node @parent_name.node
  attr (@parent_name.node) kind = "CLASS"
  attr (@parent_name.node) name = (source-text @parent_name)
  attr (@parent_name.node) start_row = (start-row @parent_name)
  attr (@parent_name.node) start_col = (start-column @parent_name)
  attr (@parent_name.node) end_row = (end-row @parent_name)
  attr (@parent_name.node) end_col = (end-column @parent_name)

  edge @class_name.node -> @parent_name.node
  attr (@class_name.node -> @parent_name.node) kind = "INHERITANCE"
}

;; Calls
(call
  method: (_) @callee_any) @call_any
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
  attr (@call_any.node -> @call_any.node) col = (start-column @callee_any)
}
