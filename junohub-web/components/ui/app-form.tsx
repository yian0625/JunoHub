"use client"

import { useId, useState, type ComponentProps } from "react"

// Constraint validation is read without asking the browser to display its popup.
export function AppForm({ onSubmit, children, ...props }: ComponentProps<"form">) {
  const errorId = useId()
  const [error, setError] = useState("")
  return <form {...props} noValidate onChange={event => {
    setError("")
    for (const field of event.currentTarget.querySelectorAll('[data-form-invalid]')) {
      field.removeAttribute("aria-invalid")
      field.removeAttribute("aria-errormessage")
      field.removeAttribute("data-form-invalid")
    }
    props.onChange?.(event)
  }} onSubmit={event => {
    const invalid = Array.from(event.currentTarget.elements).find((field): field is HTMLInputElement =>
      (field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement || field instanceof HTMLSelectElement) && field.willValidate && !field.validity.valid)
    if (invalid) {
      event.preventDefault()
      const label = invalid.labels?.[0]?.textContent?.trim() || "此项"
      const message = invalid.validity.valueMissing ? `请填写${label}。` : invalid.validity.typeMismatch ? `请输入有效的${label}。` : `请检查${label}的格式。`
      setError(message)
      const target = invalid.id ? document.getElementById(invalid.id) : invalid
      target?.setAttribute("aria-invalid", "true")
      target?.setAttribute("aria-errormessage", errorId)
      target?.setAttribute("data-form-invalid", "")
      target?.focus()
      return
    }
    setError("")
    onSubmit?.(event)
  }}>
    {children}
    {error && <p id={errorId} role="alert" className="rounded-xl border border-danger-solid/25 bg-danger-subtle px-3 py-2.5 text-sm text-danger-text">{error}</p>}
  </form>
}
