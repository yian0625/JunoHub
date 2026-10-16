"use client"

import { SearchIcon } from "lucide-react"
import { Combobox, ComboboxPopup, ComboboxEmpty, ComboboxPrimitive, ComboboxItem, ComboboxList } from "@/components/coss-ui/combobox"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/coss-ui/input-group"

export interface SearchComboboxOption {
  value: string
  label: string
  searchText?: string
}

interface SearchComboboxProps {
  id: string
  label: string
  value: string
  onValueChange: (value: string) => void
  options: SearchComboboxOption[]
  placeholder: string
  emptyText?: string
  className?: string
  icon?: React.ElementType
}

export function SearchCombobox({ id, label, value, onValueChange, options, placeholder, emptyText = "没有匹配结果", className, icon: Icon = SearchIcon }: SearchComboboxProps) {
  const selected = options.find(option => option.value === value) || null
  return <Combobox
    items={options}
    value={selected}
    onValueChange={option => onValueChange(option?.value || "")}
    itemToStringLabel={option => option.label}
    itemToStringValue={option => option.value}
    isItemEqualToValue={(option, current) => option.value === current.value}
    filter={(option, query) => matches(option.searchText || option.label, query)}
    openOnInputClick
    autoHighlight
  >
    <ComboboxPrimitive.InputGroup render={<InputGroup className={className} />}>
      <ComboboxPrimitive.Input id={id} aria-label={label} placeholder={placeholder} autoComplete="off" spellCheck={false} onKeyDown={event => { if (event.key === "Escape" && event.currentTarget.getAttribute("aria-expanded") === "true") event.stopPropagation() }} render={<InputGroupInput nativeInput />} />
      <InputGroupAddon><Icon aria-hidden="true" /></InputGroupAddon>
    </ComboboxPrimitive.InputGroup>
    <ComboboxPopup className="max-w-[calc(100vw-1.5rem)]">
      <ComboboxEmpty>{emptyText}</ComboboxEmpty>
      <ComboboxList>
        {(option: SearchComboboxOption) => <ComboboxItem key={option.value} value={option} className="min-h-10 py-2 [overflow-wrap:anywhere]">{option.label}</ComboboxItem>}
      </ComboboxList>
    </ComboboxPopup>
  </Combobox>
}

function matches(value: string, query: string) {
  const normalize = (text: string) => text.normalize("NFKC").toLocaleLowerCase().replace(/[^\p{L}\p{N}]+/gu, " ").trim()
  const haystack = normalize(value)
  return normalize(query).split(/\s+/).filter(Boolean).every(word => haystack.includes(word))
}
