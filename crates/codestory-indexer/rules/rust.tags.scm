(struct_item
  (visibility_modifier) @access
  name: (type_identifier) @definition.struct)

(struct_item
  name: (type_identifier) @definition.struct)

(enum_item
  (visibility_modifier) @access
  name: (type_identifier) @definition.enum)

(enum_item
  name: (type_identifier) @definition.enum)

(union_item
  (visibility_modifier) @access
  name: (type_identifier) @definition.union)

(union_item
  name: (type_identifier) @definition.union)

(trait_item
  (visibility_modifier) @access
  name: (type_identifier) @definition.interface)

(trait_item
  name: (type_identifier) @definition.interface)

(type_item
  (visibility_modifier) @access
  name: (type_identifier) @definition.typedef)

(type_item
  name: (type_identifier) @definition.typedef)

(macro_definition
  name: (identifier) @definition.macro)

(function_item
  (visibility_modifier) @access
  name: (identifier) @definition.function)

(function_item
  name: (identifier) @definition.function)

(impl_item
  body: (declaration_list
    (function_item
      (visibility_modifier) @access
      name: (identifier) @definition.method)))

(impl_item
  body: (declaration_list
    (function_item
      name: (identifier) @definition.method)))
