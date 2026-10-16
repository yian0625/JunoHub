"use client"

import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { cn } from "@/lib/utils"

export interface SelectOption {
  value: string
  label: string
  disabled?: boolean
}

interface SelectFieldProps {
  id: string
  name?: string
  value: string
  onValueChange: (value: string) => void
  options: SelectOption[]
  placeholder?: string
  required?: boolean
  disabled?: boolean
  className?: string
  size?: "default" | "search"
  "aria-invalid"?: boolean
  "aria-describedby"?: string
}

export function SelectField({ id, name, value, onValueChange, options, placeholder = "请选择", required, disabled, className, size = "default", "aria-invalid": invalid, "aria-describedby": describedBy }: SelectFieldProps) {
  // This selector has no clear action. Item registration changes may emit null;
  // treating that as a user choice would erase the controlled form value.
  return <Select name={name} value={value || null} onValueChange={next => { if (next !== null && next !== value) onValueChange(next) }} items={options} required={required} disabled={disabled}>
    <SelectTrigger id={id} size={size} aria-invalid={invalid} aria-describedby={describedBy} className={cn(size === "search" ? "w-full text-start shadow-xs/5" : "field-control h-11 w-full px-3.5 text-start text-base sm:text-sm", className)}>
      <SelectValue placeholder={placeholder} className="min-w-0 flex-1 truncate data-placeholder:text-muted-foreground" />
    </SelectTrigger>
    <SelectContent align="start" alignItemWithTrigger={false} sideOffset={6} className="max-w-[calc(100vw-1.5rem)]">
      {options.map(option => <SelectItem key={option.value} value={option.value} disabled={option.disabled} className="min-h-10 py-2">{option.label}</SelectItem>)}
    </SelectContent>
  </Select>
}
