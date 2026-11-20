import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AppUpdateInfo } from "../types/qwitch";

const UPDATE_CHECK_CACHE_MS = 10 * 60_000;

/** 软件更新检查：10 分钟缓存 + 并发合并 + 静默模式。 */
export function useAppUpdate(isTauriRuntime: boolean) {
  const [updateInfo, setUpdateInfo] = useState<AppUpdateInfo | null>(null);
  const [updateChecking, setUpdateChecking] = useState(false);
  const [updateError, setUpdateError] = useState<string | null>(null);
  const updateInfoRef = useRef<AppUpdateInfo | null>(null);
  updateInfoRef.current = updateInfo;
  const lastUpdateCheckAtRef = useRef(0);
  const updateCheckInFlightRef = useRef<Promise<AppUpdateInfo | null> | null>(null);

  const checkAppUpdate = useCallback(
    async (options?: { force?: boolean; silent?: boolean }): Promise<AppUpdateInfo | null> => {
      if (!isTauriRuntime) return null;
      const force = Boolean(options?.force);
      const silent = Boolean(options?.silent);
      if (
        !force &&
        updateInfoRef.current &&
        Date.now() - lastUpdateCheckAtRef.current < UPDATE_CHECK_CACHE_MS
      ) {
        return updateInfoRef.current;
      }
      if (updateCheckInFlightRef.current) {
        return updateCheckInFlightRef.current;
      }

      setUpdateChecking(true);
      if (!silent) {
        setUpdateError(null);
      }

      const promise = invoke<AppUpdateInfo>("check_app_update")
        .then((info) => {
          updateInfoRef.current = info;
          lastUpdateCheckAtRef.current = Date.now();
          setUpdateInfo(info);
          setUpdateError(null);
          return info;
        })
        .catch((err) => {
          if (!silent) {
            setUpdateError(String(err));
          }
          return null;
        })
        .finally(() => {
          updateCheckInFlightRef.current = null;
          setUpdateChecking(false);
        });

      updateCheckInFlightRef.current = promise;
      return promise;
    },
    [isTauriRuntime]
  );

  return { updateInfo, updateChecking, updateError, checkAppUpdate };
}
