import { useCallback, useEffect, useState } from "react";
import { useBridge } from "./BridgeContext";
import { errorCode, ok } from "./bridge";

interface State<T> {
  data: T | null;
  error: string | null;
}

/** GET `path` on mount and whenever `reload()` is called. Stale responses are ignored. */
export function useApi<T>(path: string) {
  const bridge = useBridge();
  const [state, setState] = useState<State<T>>({ data: null, error: null });
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let current = true;
    void bridge.api<T>("GET", path).then((res) => {
      if (!current) return;
      setState(ok(res) ? { data: res.body, error: null } : { data: null, error: errorCode(res) });
    });
    return () => {
      current = false;
    };
  }, [bridge, path, tick]);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  return { ...state, reload };
}
