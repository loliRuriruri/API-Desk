import { useCallback, useEffect, useRef, useState } from "react";
import { useToast } from "./ToastProvider";
import { errorMessage } from "../lib/errors";
import {
  decodePreview,
  exportTurzxPreview,
  fetchTurzxPreview,
  pageLabel,
  type TurzxPreview as TurzxPreviewData,
} from "../lib/turzxDisplay";

type PreviewMode = "live" | "sample";

export function TurzxPreview({ enabled }: { enabled: boolean }) {
  const { notify } = useToast();
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [mode, setMode] = useState<PreviewMode>("sample");
  const [page, setPage] = useState<string | undefined>(undefined);
  const [preview, setPreview] = useState<TurzxPreviewData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);

  const load = useCallback(async () => {
    try {
      const next = await fetchTurzxPreview(mode, page);
      setPreview(next);
      setError(null);
    } catch (cause) {
      setError(String(cause));
    }
  }, [mode, page]);

  useEffect(() => {
    let active = true;
    const tick = async () => {
      try {
        const next = await fetchTurzxPreview(mode, page);
        if (!active) return;
        setPreview(next);
        setError(null);
      } catch (cause) {
        if (active) setError(String(cause));
      }
    };
    void tick();
    return () => {
      active = false;
    };
  }, [mode, page]);

  useEffect(() => {
    if (!preview || !canvasRef.current) return;
    const canvas = canvasRef.current;
    canvas.width = preview.width;
    canvas.height = preview.height;
    const context = canvas.getContext("2d");
    if (!context) return;
    const rgba = decodePreview(preview);
    const image = context.createImageData(preview.width, preview.height);
    image.data.set(rgba);
    context.putImageData(image, 0, 0);
  }, [preview]);

  return (
    <div className="turzx-preview">
      <div className="turzx-preview-toolbar">
        <div className="turzx-preview-modes">
          <button
            type="button"
            className={mode === "sample" ? "button-small button-on" : "button-small"}
            onClick={() => {
              setPage(undefined);
              setMode("sample");
            }}
          >
            샘플
          </button>
          <button
            type="button"
            className={mode === "live" ? "button-small button-on" : "button-small"}
            disabled={!enabled}
            onClick={() => {
              setPage(undefined);
              setMode("live");
            }}
          >
            실시간
          </button>
        </div>
        <div className="turzx-preview-pages">
          {[undefined, "runtime", "api"].map((value) => (
            <button
              key={value ?? "single"}
              type="button"
              className={page === value ? "button-small button-on" : "button-small"}
              onClick={() => setPage(value)}
            >
              {value ? pageLabel(value) : "단일"}
            </button>
          ))}
        </div>
        <button type="button" className="button-ghost button-small" onClick={() => void load()}>
          새로고침
        </button>
        <button
          type="button"
          className="button-small"
          disabled={exporting}
          onClick={() => {
            setExporting(true);
            void exportTurzxPreview()
              .then((path) => notify(`미리보기 PNG 저장: ${path}`, "info"))
              .catch((cause) => notify(errorMessage(cause), "error"))
              .finally(() => setExporting(false));
          }}
        >
          {exporting ? "내보내는 중…" : "PNG 내보내기"}
        </button>
      </div>
      {error ? <p className="turzx-preview-error">{error}</p> : null}
      <canvas ref={canvasRef} className="turzx-preview-canvas" />
      <p className="turzx-preview-meta">
        {preview
          ? `${preview.width}×${preview.height} · ${mode === "live" ? "OS 실데이터" : "샘플 데이터"} · ${pageLabel(preview.page)}`
          : "미리보기 불러오는 중…"}
      </p>
    </div>
  );
}
