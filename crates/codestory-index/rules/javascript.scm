(function_declaration
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

(class_declaration
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

(method_definition
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

;; Membership
(class_declaration
  name: (identifier) @class_name
  body: (class_body
    (method_definition name: (_) @method_name)))
{
  edge @class_name.node -> @method_name.node
  attr (@class_name.node -> @method_name.node) kind = "MEMBER"
}

;; Inheritance
(class_declaration
  name: (identifier) @class_name
  (class_heritage (identifier) @parent_name))
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

(class_declaration
  name: (identifier) @class_name
  (class_heritage
    (member_expression
      property: (property_identifier) @parent_name)))
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


;; Calls (global fallback member)
(call_expression
  function: (member_expression
    property: (property_identifier) @callee_any) @call_any)
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

;; JSX component and prop usage from render paths
(function_declaration
  name: (identifier) @caller_name
  body: (statement_block
    (return_statement
      (jsx_self_closing_element
        name: (_) @component)))) @def
{
  node @caller_name.node
  attr (@caller_name.node) kind = "FUNCTION"
  attr (@caller_name.node) name = (source-text @caller_name)
  attr (@caller_name.node) start_row = (start-row @def)
  attr (@caller_name.node) start_col = (start-column @def)
  attr (@caller_name.node) end_row = (end-row @def)
  attr (@caller_name.node) end_col = (end-column @def)

  node @component.node
  attr (@component.node) kind = "UNKNOWN"
  attr (@component.node) name = (source-text @component)
  attr (@component.node) start_row = (start-row @component)
  attr (@component.node) start_col = (start-column @component)
  attr (@component.node) end_row = (end-row @component)
  attr (@component.node) end_col = (end-column @component)

  edge @caller_name.node -> @component.node
  attr (@caller_name.node -> @component.node) kind = "USAGE"
}

(function_declaration
  name: (identifier) @caller_name
  body: (statement_block
    (return_statement
      (jsx_self_closing_element
        (jsx_attribute
          (property_identifier) @attribute))))) @def
{
  node @caller_name.node
  attr (@caller_name.node) kind = "FUNCTION"
  attr (@caller_name.node) name = (source-text @caller_name)
  attr (@caller_name.node) start_row = (start-row @def)
  attr (@caller_name.node) start_col = (start-column @def)
  attr (@caller_name.node) end_row = (end-row @def)
  attr (@caller_name.node) end_col = (end-column @def)

  node @attribute.node
  attr (@attribute.node) kind = "FIELD"
  attr (@attribute.node) name = (source-text @attribute)
  attr (@attribute.node) start_row = (start-row @attribute)
  attr (@attribute.node) start_col = (start-column @attribute)
  attr (@attribute.node) end_row = (end-row @attribute)
  attr (@attribute.node) end_col = (end-column @attribute)

  edge @caller_name.node -> @attribute.node
  attr (@caller_name.node -> @attribute.node) kind = "USAGE"
}

;; Imports
(import_statement
  source: (string) @module)
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

(import_statement
  (import_clause
    (named_imports
      (import_specifier name: (identifier) @module))))
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

;; Lambda assignment
(variable_declarator
  name: (identifier) @name
  value: (arrow_function) @def)
{
  node @name.node
  attr (@name.node) kind = "FUNCTION"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

;; Import aliases
(import_statement
  (import_clause
    (identifier) @alias_name))
{
  node @alias_name.node
  attr (@alias_name.node) kind = "MODULE"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @alias_name.node -> @alias_name.node
  attr (@alias_name.node -> @alias_name.node) kind = "IMPORT"
}

(import_statement
  (import_clause
    (namespace_import (identifier) @alias_name)))
{
  node @alias_name.node
  attr (@alias_name.node) kind = "MODULE"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @alias_name.node -> @alias_name.node
  attr (@alias_name.node -> @alias_name.node) kind = "IMPORT"
}

(import_statement
  (import_clause
    (named_imports
      (import_specifier alias: (identifier) @alias_name))))
{
  node @alias_name.node
  attr (@alias_name.node) kind = "MODULE"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @alias_name.node -> @alias_name.node
  attr (@alias_name.node -> @alias_name.node) kind = "IMPORT"
}
