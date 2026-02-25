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
  declarator: (function_declarator
    declarator: (_) @name)) @def
{
  node @name.node
  attr (@name.node) kind = "METHOD"
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
  attr (@name.node) kind = "FIELD"
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

(class_specifier
  name: (_) @class_name
  body: (field_declaration_list
    (field_declaration
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

;; Inheritance
(class_specifier
  name: (_) @class_name
  (base_class_clause (_) @parent_name))
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

;; Lambda assignment
(init_declarator
  declarator: (identifier) @name
  value: (lambda_expression) @def)
{
  node @name.node
  attr (@name.node) kind = "FUNCTION"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

;; Namespace alias / using imports
(namespace_alias_definition
  name: (namespace_identifier) @alias_name
  (namespace_identifier) @target_name)
{
  node @target_name.node
  attr (@target_name.node) kind = "MODULE"
  attr (@target_name.node) name = (source-text @target_name)
  attr (@target_name.node) start_row = (start-row @target_name)
  attr (@target_name.node) start_col = (start-column @target_name)
  attr (@target_name.node) end_row = (end-row @target_name)
  attr (@target_name.node) end_col = (end-column @target_name)

  node @alias_name.node
  attr (@alias_name.node) kind = "MODULE"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @alias_name.node -> @target_name.node
  attr (@alias_name.node -> @target_name.node) kind = "IMPORT"
}

(namespace_alias_definition
  name: (namespace_identifier) @alias_name
  (nested_namespace_specifier) @target_name)
{
  node @target_name.node
  attr (@target_name.node) kind = "MODULE"
  attr (@target_name.node) name = (source-text @target_name)
  attr (@target_name.node) start_row = (start-row @target_name)
  attr (@target_name.node) start_col = (start-column @target_name)
  attr (@target_name.node) end_row = (end-row @target_name)
  attr (@target_name.node) end_col = (end-column @target_name)

  node @alias_name.node
  attr (@alias_name.node) kind = "MODULE"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @alias_name.node -> @target_name.node
  attr (@alias_name.node -> @target_name.node) kind = "IMPORT"
}

(using_declaration
  (identifier) @module)
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

(using_declaration
  (qualified_identifier) @module)
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

;; Template/type arguments
(template_type
  name: (type_identifier) @template_name
  arguments: (template_argument_list
    (type_descriptor) @type_arg))
{
  node @template_name.node
  attr (@template_name.node) kind = "CLASS"
  attr (@template_name.node) name = (source-text @template_name)
  attr (@template_name.node) start_row = (start-row @template_name)
  attr (@template_name.node) start_col = (start-column @template_name)
  attr (@template_name.node) end_row = (end-row @template_name)
  attr (@template_name.node) end_col = (end-column @template_name)

  node @type_arg.node
  attr (@type_arg.node) kind = "CLASS"
  attr (@type_arg.node) name = (source-text @type_arg)
  attr (@type_arg.node) start_row = (start-row @type_arg)
  attr (@type_arg.node) start_col = (start-column @type_arg)
  attr (@type_arg.node) end_row = (end-row @type_arg)
  attr (@type_arg.node) end_col = (end-column @type_arg)

  edge @template_name.node -> @type_arg.node
  attr (@template_name.node -> @type_arg.node) kind = "TYPE_ARGUMENT"
}

;; Calls (template function/method)
(function_definition
  declarator: (function_declarator
    declarator: (_) @caller)
  body: (compound_statement
    (expression_statement
      (call_expression
        function: (template_function name: (identifier) @callee) @call))))
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

(function_definition
  declarator: (function_declarator
    declarator: (_) @caller)
  body: (compound_statement
    (expression_statement
      (call_expression
        function: (field_expression
          field: (template_method name: (field_identifier) @callee)) @call))))
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

(call_expression
  function: (template_function name: (identifier) @callee_any) @call_any)
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
