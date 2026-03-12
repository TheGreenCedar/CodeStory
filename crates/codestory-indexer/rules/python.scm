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

(class_definition
  name: (identifier) @class_name
  body: (block
    (decorated_definition
      definition: (function_definition
        name: (identifier) @method_name))))
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

;; Calls (global fallback identifier)
(call
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

;; Calls (global fallback attribute)
(call
  function: (attribute
    attribute: (identifier) @callee_any) @call_any)
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

;; Decorator usage (class)
(decorated_definition
  (decorator
    (identifier) @decorator .)
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
  attr (@target.node -> @decorator.node) kind = "CALL"
}

;; Decorator usage (function)
(decorated_definition
  (decorator
    (identifier) @decorator .)
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
  attr (@target.node -> @decorator.node) kind = "CALL"
}

;; Decorator usage (class call)
(decorated_definition
  (decorator
    (call
      function: (identifier) @decorator))
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
  attr (@target.node -> @decorator.node) kind = "CALL"
}

;; Decorator usage (function call)
(decorated_definition
  (decorator
    (call
      function: (identifier) @decorator))
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
  attr (@target.node -> @decorator.node) kind = "CALL"
}

;; Imports
(import_from_statement
  module_name: (dotted_name) @module)
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

}

(import_from_statement
  module_name: (relative_import) @module)
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

}

(import_from_statement
  module_name: (dotted_name) @module
  name: (dotted_name) @name)
{
  node @name.node
  attr (@name.node) kind = "UNKNOWN"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @name)
  attr (@name.node) start_col = (start-column @name)
  attr (@name.node) end_row = (end-row @name)
  attr (@name.node) end_col = (end-column @name)

  edge @name.node -> @module.node
  attr (@name.node -> @module.node) kind = "IMPORT"
}

(import_from_statement
  module_name: (relative_import) @module
  name: (dotted_name) @name)
{
  node @name.node
  attr (@name.node) kind = "UNKNOWN"
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

;; Inheritance (attribute / generic parent)
(class_definition
  name: (identifier) @class_name
  superclasses: (argument_list
    (attribute) @parent_name))
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

;; Type arguments
(class_definition
  name: (identifier) @generic_name
  type_parameters: (type_parameter
    (type) @type_arg))
{
  node @type_arg.node
  attr (@type_arg.node) kind = "TYPE_PARAMETER"
  attr (@type_arg.node) name = (source-text @type_arg)
  attr (@type_arg.node) start_row = (start-row @type_arg)
  attr (@type_arg.node) start_col = (start-column @type_arg)
  attr (@type_arg.node) end_row = (end-row @type_arg)
  attr (@type_arg.node) end_col = (end-column @type_arg)

  edge @generic_name.node -> @type_arg.node
  attr (@generic_name.node -> @type_arg.node) kind = "MEMBER"
}

;; Import aliases
(import_statement
  name: (aliased_import
    name: (dotted_name) @module
    alias: (identifier) @alias_name))
{
  node @module.node
  attr (@module.node) kind = "MODULE"
  attr (@module.node) name = (source-text @module)
  attr (@module.node) start_row = (start-row @module)
  attr (@module.node) start_col = (start-column @module)
  attr (@module.node) end_row = (end-row @module)
  attr (@module.node) end_col = (end-column @module)

  node @alias_name.node
  attr (@alias_name.node) kind = "UNKNOWN"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @alias_name.node -> @module.node
  attr (@alias_name.node -> @module.node) kind = "IMPORT"
}

(import_from_statement
  module_name: (dotted_name) @module
  name: (aliased_import
    name: (dotted_name) @import_name
    alias: (identifier) @alias_name))
{
  node @import_name.node
  attr (@import_name.node) kind = "UNKNOWN"
  attr (@import_name.node) name = (source-text @import_name)
  attr (@import_name.node) start_row = (start-row @import_name)
  attr (@import_name.node) start_col = (start-column @import_name)
  attr (@import_name.node) end_row = (end-row @import_name)
  attr (@import_name.node) end_col = (end-column @import_name)

  node @alias_name.node
  attr (@alias_name.node) kind = "UNKNOWN"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @import_name.node -> @module.node
  attr (@import_name.node -> @module.node) kind = "IMPORT"

  edge @alias_name.node -> @import_name.node
  attr (@alias_name.node -> @import_name.node) kind = "IMPORT"
}

(import_from_statement
  module_name: (relative_import) @module
  name: (aliased_import
    name: (dotted_name) @import_name
    alias: (identifier) @alias_name))
{
  node @import_name.node
  attr (@import_name.node) kind = "UNKNOWN"
  attr (@import_name.node) name = (source-text @import_name)
  attr (@import_name.node) start_row = (start-row @import_name)
  attr (@import_name.node) start_col = (start-column @import_name)
  attr (@import_name.node) end_row = (end-row @import_name)
  attr (@import_name.node) end_col = (end-column @import_name)

  node @alias_name.node
  attr (@alias_name.node) kind = "UNKNOWN"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @import_name.node -> @module.node
  attr (@import_name.node -> @module.node) kind = "IMPORT"

  edge @alias_name.node -> @import_name.node
  attr (@alias_name.node -> @import_name.node) kind = "IMPORT"
}
