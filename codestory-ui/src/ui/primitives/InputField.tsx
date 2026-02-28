import { useId, type InputHTMLAttributes } from "react";

import { cx } from "./cx";

type InputFieldProps = InputHTMLAttributes<HTMLInputElement> & {
  label?: string;
  hint?: string;
  wrapperClassName?: string;
};

export function InputField({
  label,
  hint,
  wrapperClassName,
  id,
  className,
  ...props
}: InputFieldProps) {
  const generatedId = useId();
  const inputId = id ?? generatedId;

  return (
    <label className={cx("ui-input-field", wrapperClassName)} htmlFor={inputId}>
      {label ? <span className="ui-input-label">{label}</span> : null}
      <input id={inputId} className={cx("ui-input", className)} {...props} />
      {hint ? <small className="ui-input-hint">{hint}</small> : null}
    </label>
  );
}
