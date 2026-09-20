import { useEffect, useMemo, useRef, useState } from "react";
import { searchAll } from "../lib/db/repo";
import type { SearchResult } from "../types/domain";

interface SearchPaletteProps {
  onClose: () => void;
  onSelect: (result: SearchResult) => void;
}

const KIND_LABELS: Record<SearchResult["kind"], string> = {
  provider: "프로바이더",
  account: "계정",
  credential: "인증 정보",
  model: "모델",
  project: "프로젝트",
  tag: "태그",
};

export function SearchPalette({ onClose, onSelect }: SearchPaletteProps) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const [loading, setLoading] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    const trimmed = query.trim();
    if (trimmed.length === 0) return;
    let active = true;
    const timer = window.setTimeout(() => {
      setLoading(true);
      searchAll(trimmed)
        .then((value) => {
          if (!active) return;
          setResults(value);
          setActiveIndex(0);
        })
        .catch(() => {
          if (active) setResults([]);
        })
        .finally(() => {
          if (active) setLoading(false);
        });
    }, 120);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [query]);

  const trimmedQuery = query.trim();
  const busy = trimmedQuery.length > 0 && loading;

  const grouped = useMemo(() => {
    const map = new Map<SearchResult["kind"], SearchResult[]>();
    if (trimmedQuery.length === 0) return [];
    for (const result of results) {
      const list = map.get(result.kind) ?? [];
      list.push(result);
      map.set(result.kind, list);
    }
    return Array.from(map.entries());
  }, [results, trimmedQuery]);

  const flat = useMemo(() => grouped.flatMap(([, items]) => items), [grouped]);

  const handleKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((current) => Math.min(current + 1, flat.length - 1));
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((current) => Math.max(current - 1, 0));
      return;
    }
    if (event.key === "Enter" && flat[activeIndex]) {
      event.preventDefault();
      onSelect(flat[activeIndex]);
    }
  };

  return (
    <div className="palette-backdrop" onMouseDown={onClose}>
      <div
        className="palette"
        onMouseDown={(event) => event.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        <input
          ref={inputRef}
          className="palette-input"
          placeholder="프로바이더, 계정, 인증 정보, 모델, 프로젝트, 태그 검색…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <div className="palette-results">
          {busy && flat.length === 0 ? <div className="palette-empty">검색 중…</div> : null}
          {!busy && trimmedQuery.length > 0 && flat.length === 0 ? (
            <div className="palette-empty">검색 결과 없음</div>
          ) : null}
          {trimmedQuery.length === 0 ? (
            <div className="palette-empty">
              메타데이터만 검색합니다. 실제 키 값은 인덱싱되지 않습니다.
            </div>
          ) : null}
          {grouped.map(([kind, items]) => (
            <div key={kind} className="palette-group">
              <div className="palette-group-title">{KIND_LABELS[kind]}</div>
              {items.map((item) => {
                const index = flat.indexOf(item);
                return (
                  <button
                    key={`${item.kind}-${item.id}`}
                    type="button"
                    className={`palette-item ${index === activeIndex ? "palette-item-active" : ""}`}
                    onMouseEnter={() => setActiveIndex(index)}
                    onClick={() => onSelect(item)}
                  >
                    <span className="palette-item-label">{item.label}</span>
                    <span className="palette-item-sub">{item.sublabel}</span>
                    {item.extra ? <span className="palette-item-extra">{item.extra}</span> : null}
                  </button>
                );
              })}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
