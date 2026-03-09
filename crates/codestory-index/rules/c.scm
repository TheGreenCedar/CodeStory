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

(preproc_include
  path: (identifier) @module)
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

(union_specifier
  name: (type_identifier) @union_name) @def
{
  node @union_name.node
  attr (@union_name.node) kind = "UNION"
  attr (@union_name.node) name = (source-text @union_name)
  attr (@union_name.node) start_row = (start-row @def)
  attr (@union_name.node) start_col = (start-column @def)
  attr (@union_name.node) end_row = (end-row @def)
  attr (@union_name.node) end_col = (end-column @def)
}

(enum_specifier
  name: (type_identifier) @enum_name) @def
{
  node @enum_name.node
  attr (@enum_name.node) kind = "ENUM"
  attr (@enum_name.node) name = (source-text @enum_name)
  attr (@enum_name.node) start_row = (start-row @def)
  attr (@enum_name.node) start_col = (start-column @def)
  attr (@enum_name.node) end_row = (end-row @def)
  attr (@enum_name.node) end_col = (end-column @def)
}

(enumerator
  name: (identifier) @constant_name) @constant
{
  node @constant_name.node
  attr (@constant_name.node) kind = "ENUM_CONSTANT"
  attr (@constant_name.node) name = (source-text @constant_name)
  attr (@constant_name.node) start_row = (start-row @constant)
  attr (@constant_name.node) start_col = (start-column @constant)
  attr (@constant_name.node) end_row = (end-row @constant)
  attr (@constant_name.node) end_col = (end-column @constant)
}

;; Structs and members
(struct_specifier
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

(struct_specifier
  name: (type_identifier) @struct_name
  body: (field_declaration_list
    (field_declaration
      declarator: (field_identifier) @field_name)))
{
  node @field_name.node
  attr (@field_name.node) kind = "FIELD"
  attr (@field_name.node) name = (source-text @field_name)
  attr (@field_name.node) start_row = (start-row @field_name)
  attr (@field_name.node) start_col = (start-column @field_name)
  attr (@field_name.node) end_row = (end-row @field_name)
  attr (@field_name.node) end_col = (end-column @field_name)

  edge @struct_name.node -> @field_name.node
  attr (@struct_name.node -> @field_name.node) kind = "MEMBER"
}

(struct_specifier
  name: (type_identifier) @struct_name
  body: (field_declaration_list
    (field_declaration
      declarator: (pointer_declarator
        declarator: (field_identifier) @field_name))))
{
  node @field_name.node
  attr (@field_name.node) kind = "FIELD"
  attr (@field_name.node) name = (source-text @field_name)
  attr (@field_name.node) start_row = (start-row @field_name)
  attr (@field_name.node) start_col = (start-column @field_name)
  attr (@field_name.node) end_row = (end-row @field_name)
  attr (@field_name.node) end_col = (end-column @field_name)

  edge @struct_name.node -> @field_name.node
  attr (@struct_name.node -> @field_name.node) kind = "MEMBER"
}

(struct_specifier
  name: (type_identifier) @struct_name
  body: (field_declaration_list
    (field_declaration
      declarator: (function_declarator
        declarator: (pointer_declarator
          declarator: (field_identifier) @field_name)))))
{
  node @field_name.node
  attr (@field_name.node) kind = "FIELD"
  attr (@field_name.node) name = (source-text @field_name)
  attr (@field_name.node) start_row = (start-row @field_name)
  attr (@field_name.node) start_col = (start-column @field_name)
  attr (@field_name.node) end_row = (end-row @field_name)
  attr (@field_name.node) end_col = (end-column @field_name)

  edge @struct_name.node -> @field_name.node
  attr (@struct_name.node -> @field_name.node) kind = "MEMBER"
}

(struct_specifier
  name: (type_identifier) @struct_name
  body: (field_declaration_list
    (field_declaration
      declarator: (function_declarator
        declarator: (parenthesized_declarator
          (pointer_declarator
            declarator: (field_identifier) @field_name))))))
{
  node @field_name.node
  attr (@field_name.node) kind = "FIELD"
  attr (@field_name.node) name = (source-text @field_name)
  attr (@field_name.node) start_row = (start-row @field_name)
  attr (@field_name.node) start_col = (start-column @field_name)
  attr (@field_name.node) end_row = (end-row @field_name)
  attr (@field_name.node) end_col = (end-column @field_name)

  edge @struct_name.node -> @field_name.node
  attr (@struct_name.node -> @field_name.node) kind = "MEMBER"
}

;; Type alias usage
(type_definition
  type: (type_identifier) @target_type
  declarator: (type_identifier) @alias_name)
{
  node @target_type.node
  attr (@target_type.node) kind = "CLASS"
  attr (@target_type.node) name = (source-text @target_type)
  attr (@target_type.node) start_row = (start-row @target_type)
  attr (@target_type.node) start_col = (start-column @target_type)
  attr (@target_type.node) end_row = (end-row @target_type)
  attr (@target_type.node) end_col = (end-column @target_type)

  node @alias_name.node
  attr (@alias_name.node) kind = "CLASS"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)

  edge @alias_name.node -> @target_type.node
  attr (@alias_name.node -> @target_type.node) kind = "TYPE_USAGE"
}

(type_definition
  type: (struct_specifier)
  declarator: (type_identifier) @alias_name)
{
  node @alias_name.node
  attr (@alias_name.node) kind = "CLASS"
  attr (@alias_name.node) name = (source-text @alias_name)
  attr (@alias_name.node) start_row = (start-row @alias_name)
  attr (@alias_name.node) start_col = (start-column @alias_name)
  attr (@alias_name.node) end_row = (end-row @alias_name)
  attr (@alias_name.node) end_col = (end-column @alias_name)
}

(type_definition
  type: (struct_specifier
    !name
    body: (field_declaration_list
      (field_declaration
        declarator: (pointer_declarator
          declarator: (field_identifier) @field_name))))
  declarator: (type_identifier) @alias_name)
{
  node @field_name.node
  attr (@field_name.node) kind = "FIELD"
  attr (@field_name.node) name = (source-text @field_name)
  attr (@field_name.node) start_row = (start-row @field_name)
  attr (@field_name.node) start_col = (start-column @field_name)
  attr (@field_name.node) end_row = (end-row @field_name)
  attr (@field_name.node) end_col = (end-column @field_name)

  edge @alias_name.node -> @field_name.node
  attr (@alias_name.node -> @field_name.node) kind = "MEMBER"
}

(type_definition
  type: (struct_specifier
    !name
    body: (field_declaration_list
      (field_declaration
        declarator: (function_declarator
          declarator: (pointer_declarator
            declarator: (field_identifier) @field_name)))))
  declarator: (type_identifier) @alias_name)
{
  node @field_name.node
  attr (@field_name.node) kind = "FIELD"
  attr (@field_name.node) name = (source-text @field_name)
  attr (@field_name.node) start_row = (start-row @field_name)
  attr (@field_name.node) start_col = (start-column @field_name)
  attr (@field_name.node) end_row = (end-row @field_name)
  attr (@field_name.node) end_col = (end-column @field_name)

  edge @alias_name.node -> @field_name.node
  attr (@alias_name.node -> @field_name.node) kind = "MEMBER"
}

(type_definition
  type: (struct_specifier
    !name
    body: (field_declaration_list
      (field_declaration
        declarator: (function_declarator
          declarator: (parenthesized_declarator
            (pointer_declarator
              declarator: (field_identifier) @field_name))))))
  declarator: (type_identifier) @alias_name)
{
  node @field_name.node
  attr (@field_name.node) kind = "FIELD"
  attr (@field_name.node) name = (source-text @field_name)
  attr (@field_name.node) start_row = (start-row @field_name)
  attr (@field_name.node) start_col = (start-column @field_name)
  attr (@field_name.node) end_row = (end-row @field_name)
  attr (@field_name.node) end_col = (end-column @field_name)

  edge @alias_name.node -> @field_name.node
  attr (@alias_name.node -> @field_name.node) kind = "MEMBER"
}

(declaration
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

(call_expression
  function: (pointer_expression
    argument: (identifier) @callee_any) @call_any)
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
