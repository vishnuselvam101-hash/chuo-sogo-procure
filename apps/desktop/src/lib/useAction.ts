import { useCallback, useState } from "react";
import { useBridge } from "./BridgeContext";
import { errorCode, ok, type HttpMethod } from "./bridge";

/**
 * One mutating API call at a time, with its error code for <ErrorText>.
 * `run` resolves to `{ body }` on success (body may be null for 204) or `null` on failure.
 */
export function useAction() {
  const bridge = useBridge();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = useCallback(
    async <T = unknown>(method: HttpMethod, path: string, body?: unknown): Promise<{ body: T } | null> => {
      setBusy(true);
      setError(null);
      try {
        const res = await bridge.api<T>(method, path, body);
        if (ok(res)) return { body: res.body };
        setError(errorCode(res));
        return null;
      } finally {
        setBusy(false);
      }
    },
    [bridge],
  );

  return { error, busy, run, setError };
}
