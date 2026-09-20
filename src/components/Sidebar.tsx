import mikuIcon from "../assets/miku-icon.png";

export type PageKey = "dashboard" | "apis" | "models" | "projects" | "settings" | "import";

const NAV_ITEMS: Array<{ key: PageKey; label: string; hint: string }> = [
  { key: "dashboard", label: "대시보드", hint: "전체 요약" },
  { key: "apis", label: "API 관리", hint: "프로바이더 · 계정 · 키" },
  { key: "models", label: "모델", hint: "모델 목록" },
  { key: "projects", label: "프로젝트", hint: "사용 매핑" },
  { key: "settings", label: "설정", hint: "환경 설정" },
];

interface SidebarProps {
  page: PageKey;
  onNavigate: (page: PageKey) => void;
  onLockVault: () => void;
  locked: boolean;
}

export function Sidebar({ page, onNavigate, onLockVault, locked }: SidebarProps) {
  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <img className="brand-mark-img" src={mikuIcon} alt="" />
        <div>
          <strong>API Desk</strong>
          <span className="brand-version">v0.1</span>
        </div>
      </div>
      <nav className="sidebar-nav">
        {NAV_ITEMS.map((item) => (
          <button
            key={item.key}
            type="button"
            className={`nav-item ${page === item.key ? "nav-item-active" : ""}`}
            onClick={() => onNavigate(item.key)}
          >
            <span>{item.label}</span>
            <small>{item.hint}</small>
          </button>
        ))}
      </nav>
      <div className="sidebar-footer">
        <span className="vault-status">
          <span className="vault-dot" />
          {locked ? "Vault 잠김" : "Vault 열림"}
        </span>
        <button
          type="button"
          className="button button-small button-ghost"
          onClick={onLockVault}
          disabled={locked}
        >
          {locked ? "잠금 해제 필요" : "잠그기"}
        </button>
      </div>
    </aside>
  );
}
