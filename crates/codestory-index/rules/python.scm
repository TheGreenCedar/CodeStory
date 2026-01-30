(function_definition
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

(class_definition
  name: (identifier) @name)
{
  node @name.node
  attr (@name.node) kind = "CLASS"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

(assignment
  left: (identifier) @name)
{
  node @name.node
  attr (@name.node) kind = "VARIABLE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)
}

;; Membership
(class_definition
  name: (identifier) @class_name
  body: (block
    (function_definition
      name: (identifier) @method_name)))
{
  edge @class_name.node -> @method_name.node
  attr (@class_name.node -> @method_name.node) kind = "MEMBER"
}

;; Inheritance
(class_definition
  name: (identifier) @class_name
  superclasses: (argument_list
    (identifier) @parent_name))
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

;; Calls (identifier)
(function_definition
  name: (identifier) @caller
  body: (block
    (expression_statement
      (call function: (identifier) @callee) @call)))
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

;; Calls (attribute)
(function_definition
  name: (identifier) @caller
  body: (block
    (expression_statement
      (call function: (attribute
        attribute: (identifier) @callee) @call))))
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

;; Decorator usage (class)
(decorated_definition
  (decorator (identifier) @decorator)
  definition: (class_definition name: (identifier) @target))
{
  node @decorator.node
  attr (@decorator.node) kind = "FUNCTION"
  attr (@decorator.node) name = (source-text @decorator)
  attr (@decorator.node) start_row = (start-row @decorator)
  attr (@decorator.node) start_col = (start-column @decorator)
  attr (@decorator.node) end_row = (end-row @decorator)
  attr (@decorator.node) end_col = (end-column @decorator)

  edge @target.node -> @decorator.node
  attr (@target.node -> @decorator.node) kind = "USAGE"
}

;; Decorator usage (function)
(decorated_definition
  (decorator (identifier) @decorator)
  definition: (function_definition name: (identifier) @target))
{
  node @decorator.node
  attr (@decorator.node) kind = "FUNCTION"
  attr (@decorator.node) name = (source-text @decorator)
  attr (@decorator.node) start_row = (start-row @decorator)
  attr (@decorator.node) start_col = (start-column @decorator)
  attr (@decorator.node) end_row = (end-row @decorator)
  attr (@decorator.node) end_col = (end-column @decorator)

  edge @target.node -> @decorator.node
  attr (@target.node -> @decorator.node) kind = "USAGE"
}

;; Imports
(import_from_statement
  module_name: (dotted_name) @module
  name: (dotted_name) @name)
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

  node @name.node
  attr (@name.node) kind = "MODULE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)

  edge @name.node -> @module.node
  attr (@name.node -> @module.node) kind = "IMPORT"
}

(import_statement
  name: (dotted_name) @name)
{
  node @name.node
  attr (@name.node) kind = "MODULE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)

  edge @name.node -> @name.node
  attr (@name.node -> @name.node) kind = "IMPORT"
}
