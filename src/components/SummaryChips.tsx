interface SummaryChipsProps {
  text: string;
  className?: string;
}

export function SummaryChips({ text, className = "" }: SummaryChipsProps) {
  const parts = text
    .split(" · ")
    .map((part) => part.trim())
    .filter((part) => part.length > 0);
  if (parts.length === 0) return null;
  return (
    <div className={`summary-chips ${className}`.trim()}>
      {parts.map((part, index) => (
        <span key={`${index}:${part}`} className="summary-chip">
          {part}
        </span>
      ))}
    </div>
  );
}
