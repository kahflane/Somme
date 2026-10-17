import { useState, type KeyboardEvent } from "react";
import classes from "@/presentation/common/floating-field.module.css";

interface Props {
  label: string;
  value: string;
  onChange?: (value: string) => void;
  onBlur?: () => void;
  onKeyDown?: (event: KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>) => void;
  required?: boolean;
  multiline?: boolean;
  password?: boolean;
  readOnly?: boolean;
  mono?: boolean;
  autoFocus?: boolean;
  // Always shown, so the label sits on the border.
  placeholder?: string;
  // Shown only while the field has focus.
  hint?: string;
  error?: string | null;
}

// An outlined field whose label sits inside while empty and moves onto the
// border once the field has focus or a value.
export function FloatingField({
  label,
  value,
  onChange,
  onBlur,
  onKeyDown,
  required,
  multiline,
  password,
  readOnly,
  mono,
  autoFocus,
  placeholder,
  hint,
  error,
}: Props) {
  const [focused, setFocused] = useState(false);
  const floating = focused || value.length > 0 || Boolean(placeholder);
  const shared = {
    className: classes.input,
    value,
    readOnly,
    autoFocus,
    spellCheck: false,
    placeholder: placeholder ?? (focused ? hint : undefined),
    "data-mono": mono || undefined,
    "aria-label": label,
    "aria-invalid": error ? true : undefined,
    onFocus: () => setFocused(true),
    onBlur: () => {
      setFocused(false);
      onBlur?.();
    },
    onKeyDown,
  };
  return (
    <div>
      <label
        className={classes.field}
        data-floating={floating || undefined}
        data-error={error ? true : undefined}
        data-readonly={readOnly || undefined}
      >
        <span className={classes.label}>
          {label}
          {required && " *"}
        </span>
        {multiline ? (
          <textarea {...shared} onChange={(event) => onChange?.(event.currentTarget.value)} />
        ) : (
          <input
            {...shared}
            type={password ? "password" : "text"}
            onChange={(event) => onChange?.(event.currentTarget.value)}
          />
        )}
      </label>
      {error && <div className={classes.error}>{error}</div>}
    </div>
  );
}
