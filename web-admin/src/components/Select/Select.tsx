import { useState, useRef, useEffect, useLayoutEffect, useCallback, useMemo } from 'react'
import { createPortal } from 'react-dom'
import { ChevronDown, Check, Search } from 'lucide-react'
import './Select.css'

export interface SelectOption {
  label: React.ReactNode
  value: string | number
  /** 可搜索文本（label 为 ReactNode 时使用） */
  searchText?: string
}

interface SelectProps {
  value: string | number | undefined
  options: SelectOption[]
  placeholder?: string
  disabled?: boolean
  editable?: boolean
  searchable?: boolean
  emptyText?: string
  onChange: (value: string | number) => void
}

export function Select({ value, options, placeholder = '请选择', disabled = false, editable = false, searchable = false, emptyText, onChange }: SelectProps) {
  const [open, setOpen] = useState(false)
  const [inputValue, setInputValue] = useState<string>('')
  const [searchKeyword, setSearchKeyword] = useState<string>('')
  const ref = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const dropdownRef = useRef<HTMLDivElement>(null)
  const [dropdownStyle, setDropdownStyle] = useState<React.CSSProperties>({
    position: 'fixed',
    visibility: 'hidden',
  })

  const selected = options.find((o) => o.value === value)

  useEffect(() => {
    const sel = options.find((o) => o.value === value)
    setInputValue(sel ? String(sel.label) : (value != null ? String(value) : ''))
  }, [value, options])

  // 搜索过滤 + 限制最多100条
  const filteredOptions = useMemo(() => {
    const maxItems = searchable ? 300 : 100
    if (!searchable || !searchKeyword) {
      return options.slice(0, maxItems)
    }
    const kw = searchKeyword.toLowerCase()
    return options.filter((o) => {
      const haystack = (o.searchText ?? String(o.label)).toLowerCase()
      const val = String(o.value).toLowerCase()
      return haystack.includes(kw) || val.includes(kw)
    }).slice(0, maxItems)
  }, [options, searchable, searchKeyword])

  const updatePosition = useCallback(() => {
    if (!ref.current) return
    const rect = ref.current.getBoundingClientRect()
    const maxHeight = 320
    const gap = 4
    const spaceBelow = window.innerHeight - rect.bottom - gap
    const spaceAbove = rect.top - gap
    const openUpward = spaceBelow < Math.min(maxHeight, 200) && spaceAbove > spaceBelow
    const top = openUpward ? Math.max(gap, rect.top - gap - maxHeight) : rect.bottom + gap
    setDropdownStyle({
      position: 'fixed',
      top,
      left: rect.left,
      width: rect.width,
      zIndex: 99999,
      visibility: 'visible',
    })
  }, [])

  const openDropdown = useCallback(() => {
    if (disabled) return
    updatePosition()
    setOpen(true)
  }, [disabled, updatePosition])

  const closeDropdown = useCallback(() => {
    setOpen(false)
    setSearchKeyword('')
    setDropdownStyle((current) => ({ ...current, visibility: 'hidden' }))
  }, [])

  const toggleDropdown = useCallback(() => {
    if (disabled) return
    if (open) {
      closeDropdown()
    } else {
      openDropdown()
    }
  }, [closeDropdown, disabled, open, openDropdown])

  useLayoutEffect(() => {
    if (!open) return
    updatePosition()
  }, [open, updatePosition])

  useEffect(() => {
    if (!open) return
    const handler = (e: MouseEvent) => {
      const target = e.target as Node
      if (ref.current && ref.current.contains(target)) return
      if (dropdownRef.current && dropdownRef.current.contains(target)) return
      closeDropdown()
    }
    document.addEventListener('mousedown', handler, true)
    return () => document.removeEventListener('mousedown', handler, true)
  }, [open, closeDropdown])

  useEffect(() => {
    if (!open) return
    const onScroll = () => updatePosition()
    const onResize = () => updatePosition()
    window.addEventListener('scroll', onScroll, true)
    window.addEventListener('resize', onResize)
    return () => {
      window.removeEventListener('scroll', onScroll, true)
      window.removeEventListener('resize', onResize)
    }
  }, [open, updatePosition])

  // searchable模式打开时自动聚焦搜索框
  useEffect(() => {
    if (open && searchable && searchRef.current) {
      requestAnimationFrame(() => searchRef.current?.focus())
    }
    if (!open) {
      setSearchKeyword('')
    }
  }, [open, searchable])

  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setInputValue(e.target.value)
    onChange(e.target.value)
  }

  const handleInputFocus = () => {
    if (editable) openDropdown()
  }

  return (
    <div ref={ref} className={`select ${open ? 'select--open' : ''} ${disabled ? 'select--disabled' : ''} ${editable ? 'select--editable' : ''} ${searchable ? 'select--searchable' : ''}`}>
      {editable ? (
        <div className="select__trigger">
          <input
            ref={inputRef}
            type="text"
            className="select__input"
            value={inputValue}
            placeholder={placeholder}
            disabled={disabled}
            onChange={handleInputChange}
            onFocus={handleInputFocus}
          />
          <ChevronDown size={16} className={`select__arrow ${open ? 'select__arrow--up' : ''}`} onClick={toggleDropdown} />
        </div>
      ) : (
        <button
          type="button"
          className="select__trigger"
          onClick={toggleDropdown}
          aria-haspopup="listbox"
          aria-expanded={open}
        >
          <span className={selected ? 'select__value' : 'select__placeholder'}>
            {selected ? selected.label : placeholder}
          </span>
          <ChevronDown size={16} className={`select__arrow ${open ? 'select__arrow--up' : ''}`} />
        </button>
      )}

      {open && createPortal(
        <div ref={dropdownRef} className="select__dropdown" role="listbox" style={dropdownStyle}>
          {searchable && (
            <div className="select__search">
              <Search size={14} className="select__search-icon" />
              <input
                ref={searchRef}
                type="text"
                className="select__search-input"
                placeholder="搜索..."
                value={searchKeyword}
                onChange={(e) => setSearchKeyword(e.target.value)}
              />
            </div>
          )}
          <div className="select__list">
            {filteredOptions.length === 0 ? (
              <div className="select__empty">{emptyText || '无匹配结果'}</div>
            ) : (
              filteredOptions.map((opt) => (
                <div
                  key={opt.value}
                  role="option"
                  aria-selected={value === opt.value}
                  className={`select__option ${value === opt.value ? 'select__option--active' : ''}`}
                  onClick={() => {
                    onChange(opt.value)
                    setInputValue(opt.searchText ?? (typeof opt.label === 'string' ? opt.label : String(opt.value)))
                    closeDropdown()
                  }}
                >
                  <span>{opt.label}</span>
                  {value === opt.value && <Check size={16} />}
                </div>
              ))
            )}
          </div>
        </div>,
        document.body
      )}
    </div>
  )
}
