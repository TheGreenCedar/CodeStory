(method_declaration
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "METHOD"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(class_declaration
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "CLASS"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(interface_declaration
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "INTERFACE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(record_declaration
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "CLASS"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(enum_declaration
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "ENUM"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(annotation_type_declaration
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "ANNOTATION"
  attr (@name.node) canonical_role = "declaration"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(enum_constant
  name: (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "ENUM_CONSTANT"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(field_declaration
  (variable_declarator name: (identifier) @name)) @def
{
  node @name.node
  attr (@name.node) kind = "FIELD"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(package_declaration
  (scoped_identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "MODULE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(package_declaration
  (identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "MODULE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(module_declaration
  name: (scoped_identifier) @name) @def
{
  node @name.node
  attr (@name.node) kind = "MODULE"
  attr (@name.node) name = (source-text @name)
  attr (@name.node) start_row = (start-row @def)
  attr (@name.node) start_col = (start-column @def)
  attr (@name.node) end_row = (end-row @def)
  attr (@name.node) end_col = (end-column @def)
}

(module_declaration
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

;; Membership (methods)
(class_declaration
  name: (identifier) @class_name
  body: (class_body
    (method_declaration name: (identifier) @method_name)))
{
  edge @class_name.node -> @method_name.node
  attr (@class_name.node -> @method_name.node) kind = "MEMBER"
}

(interface_declaration
  name: (identifier) @interface_name
  body: (interface_body
    (method_declaration name: (identifier) @method_name)))
{
  edge @interface_name.node -> @method_name.node
  attr (@interface_name.node -> @method_name.node) kind = "MEMBER"
}

(class_declaration
  name: (identifier) @class_name
  body: (class_body
    (constructor_declaration name: (identifier) @method_name) @method_def))
{
  node @method_name.node
  attr (@method_name.node) kind = "METHOD"
  attr (@method_name.node) name = (source-text @method_name)
  attr (@method_name.node) start_row = (start-row @method_def)
  attr (@method_name.node) start_col = (start-column @method_def)
  attr (@method_name.node) end_row = (end-row @method_def)
  attr (@method_name.node) end_col = (end-column @method_def)

  edge @class_name.node -> @method_name.node
  attr (@class_name.node -> @method_name.node) kind = "MEMBER"
}

(record_declaration
  name: (identifier) @class_name
  body: (class_body
    (constructor_declaration name: (identifier) @method_name) @method_def))
{
  node @method_name.node
  attr (@method_name.node) kind = "METHOD"
  attr (@method_name.node) name = (source-text @method_name)
  attr (@method_name.node) start_row = (start-row @method_def)
  attr (@method_name.node) start_col = (start-column @method_def)
  attr (@method_name.node) end_row = (end-row @method_def)
  attr (@method_name.node) end_col = (end-column @method_def)

  edge @class_name.node -> @method_name.node
  attr (@class_name.node -> @method_name.node) kind = "MEMBER"
}

(record_declaration
  name: (identifier) @class_name
  body: (class_body
    (compact_constructor_declaration name: (identifier) @method_name) @method_def))
{
  node @method_name.node
  attr (@method_name.node) kind = "METHOD"
  attr (@method_name.node) name = (source-text @method_name)
  attr (@method_name.node) start_row = (start-row @method_def)
  attr (@method_name.node) start_col = (start-column @method_def)
  attr (@method_name.node) end_row = (end-row @method_def)
  attr (@method_name.node) end_col = (end-column @method_def)

  edge @class_name.node -> @method_name.node
  attr (@class_name.node -> @method_name.node) kind = "MEMBER"
}

;; Membership (fields)
(class_declaration
  name: (identifier) @class_name
  body: (class_body
    (field_declaration (variable_declarator name: (identifier) @field_name))))
{
  edge @class_name.node -> @field_name.node
  attr (@class_name.node -> @field_name.node) kind = "MEMBER"
}

(enum_declaration
  name: (identifier) @enum_name
  body: (enum_body
    (enum_constant name: (identifier) @constant_name)))
{
  edge @enum_name.node -> @constant_name.node
  attr (@enum_name.node -> @constant_name.node) kind = "MEMBER"
}

(class_declaration
  name: (identifier) @class_name
  body: (class_body
    (class_declaration name: (identifier) @member_name)))
{
  edge @class_name.node -> @member_name.node
  attr (@class_name.node -> @member_name.node) kind = "MEMBER"
}

(class_declaration
  name: (identifier) @class_name
  body: (class_body
    (interface_declaration name: (identifier) @member_name)))
{
  edge @class_name.node -> @member_name.node
  attr (@class_name.node -> @member_name.node) kind = "MEMBER"
}

(class_declaration
  name: (identifier) @class_name
  body: (class_body
    (enum_declaration name: (identifier) @member_name)))
{
  edge @class_name.node -> @member_name.node
  attr (@class_name.node -> @member_name.node) kind = "MEMBER"
}

(class_declaration
  name: (identifier) @class_name
  body: (class_body
    (record_declaration name: (identifier) @member_name)))
{
  edge @class_name.node -> @member_name.node
  attr (@class_name.node -> @member_name.node) kind = "MEMBER"
}

(class_declaration
  name: (identifier) @class_name
  body: (class_body
    (annotation_type_declaration name: (identifier) @member_name)))
{
  edge @class_name.node -> @member_name.node
  attr (@class_name.node -> @member_name.node) kind = "MEMBER"
}

(interface_declaration
  name: (identifier) @interface_name
  body: (interface_body
    (class_declaration name: (identifier) @member_name)))
{
  edge @interface_name.node -> @member_name.node
  attr (@interface_name.node -> @member_name.node) kind = "MEMBER"
}

(interface_declaration
  name: (identifier) @interface_name
  body: (interface_body
    (interface_declaration name: (identifier) @member_name)))
{
  edge @interface_name.node -> @member_name.node
  attr (@interface_name.node -> @member_name.node) kind = "MEMBER"
}

(interface_declaration
  name: (identifier) @interface_name
  body: (interface_body
    (enum_declaration name: (identifier) @member_name)))
{
  edge @interface_name.node -> @member_name.node
  attr (@interface_name.node -> @member_name.node) kind = "MEMBER"
}

(interface_declaration
  name: (identifier) @interface_name
  body: (interface_body
    (record_declaration name: (identifier) @member_name)))
{
  edge @interface_name.node -> @member_name.node
  attr (@interface_name.node -> @member_name.node) kind = "MEMBER"
}

(interface_declaration
  name: (identifier) @interface_name
  body: (interface_body
    (annotation_type_declaration name: (identifier) @member_name)))
{
  edge @interface_name.node -> @member_name.node
  attr (@interface_name.node -> @member_name.node) kind = "MEMBER"
}

(record_declaration
  name: (identifier) @record_name
  body: (class_body
    (class_declaration name: (identifier) @member_name)))
{
  edge @record_name.node -> @member_name.node
  attr (@record_name.node -> @member_name.node) kind = "MEMBER"
}

(record_declaration
  name: (identifier) @record_name
  body: (class_body
    (interface_declaration name: (identifier) @member_name)))
{
  edge @record_name.node -> @member_name.node
  attr (@record_name.node -> @member_name.node) kind = "MEMBER"
}

(record_declaration
  name: (identifier) @record_name
  body: (class_body
    (enum_declaration name: (identifier) @member_name)))
{
  edge @record_name.node -> @member_name.node
  attr (@record_name.node -> @member_name.node) kind = "MEMBER"
}

(record_declaration
  name: (identifier) @record_name
  body: (class_body
    (record_declaration name: (identifier) @member_name)))
{
  edge @record_name.node -> @member_name.node
  attr (@record_name.node -> @member_name.node) kind = "MEMBER"
}

(record_declaration
  name: (identifier) @record_name
  body: (class_body
    (annotation_type_declaration name: (identifier) @member_name)))
{
  edge @record_name.node -> @member_name.node
  attr (@record_name.node -> @member_name.node) kind = "MEMBER"
}

(enum_declaration
  name: (identifier) @enum_name
  body: (enum_body
    (enum_body_declarations
      (class_declaration name: (identifier) @member_name))))
{
  edge @enum_name.node -> @member_name.node
  attr (@enum_name.node -> @member_name.node) kind = "MEMBER"
}

(enum_declaration
  name: (identifier) @enum_name
  body: (enum_body
    (enum_body_declarations
      (interface_declaration name: (identifier) @member_name))))
{
  edge @enum_name.node -> @member_name.node
  attr (@enum_name.node -> @member_name.node) kind = "MEMBER"
}

(enum_declaration
  name: (identifier) @enum_name
  body: (enum_body
    (enum_body_declarations
      (enum_declaration name: (identifier) @member_name))))
{
  edge @enum_name.node -> @member_name.node
  attr (@enum_name.node -> @member_name.node) kind = "MEMBER"
}

(enum_declaration
  name: (identifier) @enum_name
  body: (enum_body
    (enum_body_declarations
      (record_declaration name: (identifier) @member_name))))
{
  edge @enum_name.node -> @member_name.node
  attr (@enum_name.node -> @member_name.node) kind = "MEMBER"
}

(enum_declaration
  name: (identifier) @enum_name
  body: (enum_body
    (enum_body_declarations
      (annotation_type_declaration name: (identifier) @member_name))))
{
  edge @enum_name.node -> @member_name.node
  attr (@enum_name.node -> @member_name.node) kind = "MEMBER"
}

(annotation_type_declaration
  name: (identifier) @annotation_name
  body: (annotation_type_body
    (class_declaration name: (identifier) @member_name)))
{
  edge @annotation_name.node -> @member_name.node
  attr (@annotation_name.node -> @member_name.node) kind = "MEMBER"
}

(annotation_type_declaration
  name: (identifier) @annotation_name
  body: (annotation_type_body
    (interface_declaration name: (identifier) @member_name)))
{
  edge @annotation_name.node -> @member_name.node
  attr (@annotation_name.node -> @member_name.node) kind = "MEMBER"
}

(annotation_type_declaration
  name: (identifier) @annotation_name
  body: (annotation_type_body
    (enum_declaration name: (identifier) @member_name)))
{
  edge @annotation_name.node -> @member_name.node
  attr (@annotation_name.node -> @member_name.node) kind = "MEMBER"
}

(annotation_type_declaration
  name: (identifier) @annotation_name
  body: (annotation_type_body
    (annotation_type_declaration name: (identifier) @member_name)))
{
  edge @annotation_name.node -> @member_name.node
  attr (@annotation_name.node -> @member_name.node) kind = "MEMBER"
}

;; Package membership
(program
  (package_declaration (scoped_identifier) @package_name)
  (class_declaration name: (identifier) @class_name))
{
  edge @package_name.node -> @class_name.node
  attr (@package_name.node -> @class_name.node) kind = "MEMBER"
}

(program
  (package_declaration (identifier) @package_name)
  (class_declaration name: (identifier) @class_name))
{
  edge @package_name.node -> @class_name.node
  attr (@package_name.node -> @class_name.node) kind = "MEMBER"
}

(program
  (package_declaration (scoped_identifier) @package_name)
  (interface_declaration name: (identifier) @class_name))
{
  edge @package_name.node -> @class_name.node
  attr (@package_name.node -> @class_name.node) kind = "MEMBER"
}

(program
  (package_declaration (identifier) @package_name)
  (interface_declaration name: (identifier) @class_name))
{
  edge @package_name.node -> @class_name.node
  attr (@package_name.node -> @class_name.node) kind = "MEMBER"
}

;; Inheritance (extends)
(class_declaration
  name: (identifier) @class_name
  superclass: (superclass (type_identifier) @parent_name))
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

;; Calls (global fallback)
(method_invocation
  name: (identifier) @callee_any) @call_any
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

(annotation
  name: (_) @annotation_name) @annotation_use
{
  node @annotation_name.node
  attr (@annotation_name.node) kind = "ANNOTATION"
  attr (@annotation_name.node) name = (source-text @annotation_name)
  attr (@annotation_name.node) start_row = (start-row @annotation_name)
  attr (@annotation_name.node) start_col = (start-column @annotation_name)
  attr (@annotation_name.node) end_row = (end-row @annotation_name)
  attr (@annotation_name.node) end_col = (end-column @annotation_name)

  edge @annotation_name.node -> @annotation_name.node
  attr (@annotation_name.node -> @annotation_name.node) kind = "ANNOTATION_USAGE"
  attr (@annotation_name.node -> @annotation_name.node) line = (start-row @annotation_use)
}

(marker_annotation
  name: (_) @annotation_name) @annotation_use
{
  node @annotation_name.node
  attr (@annotation_name.node) kind = "ANNOTATION"
  attr (@annotation_name.node) name = (source-text @annotation_name)
  attr (@annotation_name.node) start_row = (start-row @annotation_name)
  attr (@annotation_name.node) start_col = (start-column @annotation_name)
  attr (@annotation_name.node) end_row = (end-row @annotation_name)
  attr (@annotation_name.node) end_col = (end-column @annotation_name)

  edge @annotation_name.node -> @annotation_name.node
  attr (@annotation_name.node -> @annotation_name.node) kind = "ANNOTATION_USAGE"
  attr (@annotation_name.node -> @annotation_name.node) line = (start-row @annotation_use)
}

;; Imports
(import_declaration
  (scoped_identifier) @module)
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

(import_declaration
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

;; Lambda assignment
(variable_declarator
  name: (identifier) @name
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

;; Inheritance (superclass variants / implements / interface extends)
(class_declaration
  name: (identifier) @class_name
  superclass: (superclass (scoped_type_identifier) @parent_name))
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
  superclass: (superclass (generic_type
    (type_identifier) @parent_name)))
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
  interfaces: (super_interfaces
    (type_list
      (_) @parent_name)))
{
  node @parent_name.node
  attr (@parent_name.node) kind = "INTERFACE"
  attr (@parent_name.node) name = (source-text @parent_name)
  attr (@parent_name.node) start_row = (start-row @parent_name)
  attr (@parent_name.node) start_col = (start-column @parent_name)
  attr (@parent_name.node) end_row = (end-row @parent_name)
  attr (@parent_name.node) end_col = (end-column @parent_name)

  edge @class_name.node -> @parent_name.node
  attr (@class_name.node -> @parent_name.node) kind = "INHERITANCE"
}

(interface_declaration
  name: (identifier) @interface_name
  (extends_interfaces
    (type_list
      (_) @parent_name)))
{
  node @parent_name.node
  attr (@parent_name.node) kind = "INTERFACE"
  attr (@parent_name.node) name = (source-text @parent_name)
  attr (@parent_name.node) start_row = (start-row @parent_name)
  attr (@parent_name.node) start_col = (start-column @parent_name)
  attr (@parent_name.node) end_row = (end-row @parent_name)
  attr (@parent_name.node) end_col = (end-column @parent_name)

  edge @interface_name.node -> @parent_name.node
  attr (@interface_name.node -> @parent_name.node) kind = "INHERITANCE"
}
