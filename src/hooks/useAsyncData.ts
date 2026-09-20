import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage } from "../lib/errors";

export interface AsyncData<T> {
  data: T | null;
  loading: boolean;
  error: string | null;
  reload: () => void;
}

interface LoadResult<T> {
  requestKey: string;
  data: T | null;
  error: string | null;
}

export function useAsyncData<T>(loader: () => Promise<T>, key: string): AsyncData<T> {
  const [version, setVersion] = useState(0);
  const requestKey = `${key}:${version}`;
  const [result, setResult] = useState<LoadResult<T> | null>(null);
  const loaderRef = useRef(loader);

  useEffect(() => {
    loaderRef.current = loader;
  }, [loader]);

  useEffect(() => {
    let active = true;
    loaderRef.current()
      .then((value) => {
        if (active) setResult({ requestKey, data: value, error: null });
      })
      .catch((err: unknown) => {
        if (active) setResult({ requestKey, data: null, error: errorMessage(err) });
      });
    return () => {
      active = false;
    };
  }, [requestKey]);

  const reload = useCallback(() => setVersion((current) => current + 1), []);
  const settled = result !== null && result.requestKey === requestKey;

  return {
    data: result?.data ?? null,
    loading: !settled,
    error: settled ? (result?.error ?? null) : null,
    reload,
  };
}
