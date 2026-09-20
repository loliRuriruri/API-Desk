import mikuIcon from "../assets/miku-icon.png";

export function EmptyState({ text, hint }: { text: string; hint?: string }) {
  return (
    <div className="empty-state">
      <img className="empty-state-art" src={mikuIcon} alt="" />
      <div className="empty-state-copy">
        <span>{text}</span>
        {hint ? <small>{hint}</small> : null}
      </div>
    </div>
  );
}
