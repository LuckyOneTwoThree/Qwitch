// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MOCK_ADAPTERS } from "../../data/mockStore";
import { normalizeAppSettings } from "../../context/settingsUtils";
import type { AppSettings, ExternalAdapterInfo } from "../../types/qwitch";

const saveAppSettings = vi.fn();
const refreshClientStatuses = vi.fn();
const showToast = vi.fn();
let settings: AppSettings;
let tauri = false;

vi.mock("../../context/QwitchContext", () => ({
  useQwitch: () => ({
    adapters: MOCK_ADAPTERS,
    appSettings: settings,
    clientStatuses: {},
    isTauriRuntime: tauri,
    jitterRangeMs: [1500, 3500],
    setJitterRangeMs: vi.fn(),
    logRetention: { archiveAfterDays: 14, maxActiveCount: 200 },
    setLogRetention: vi.fn(),
    refreshClientStatuses,
    saveAppSettings,
    showToast,
  }),
}));
vi.mock("../../components/UpdateCard", () => ({ UpdateCard: () => null }));

import { SettingsView, type SettingsViewProps } from "./SettingsView";

function renderView(over: Partial<SettingsViewProps> = {}) {
  const props: SettingsViewProps = {
    autoStartDescription: "desc",
    externalPlugins: [],
    trustedDraft: [],
    setTrustedDraft: vi.fn(),
    pathDrafts: {},
    setPathDrafts: vi.fn(),
    setPathDraftsDirty: vi.fn(),
    reloadExternalPlugins: vi.fn().mockResolvedValue(undefined),
    setConfirmLogAction: vi.fn(),
    ...over,
  };
  render(<SettingsView {...props} />);
  return props;
}

beforeEach(() => {
  settings = normalizeAppSettings();
  tauri = false;
  saveAppSettings.mockReset().mockResolvedValue(true);
  refreshClientStatuses.mockReset().mockResolvedValue(undefined);
  showToast.mockReset();
});
afterEach(cleanup);

describe("SettingsView", () => {
  it("marks the active theme with aria-pressed and saves a new one", () => {
    settings = { ...settings, theme: "light" };
    renderView();
    expect(screen.getByRole("button", { name: "浅色" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "深色" }).getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(screen.getByRole("button", { name: "深色" }));
    expect(saveAppSettings).toHaveBeenCalledWith({ theme: "dark" }, true);
  });

  it("toggles close-to-tray (default on) off", () => {
    renderView();
    const sw = screen.getByRole("switch", { name: /关闭主窗口时隐藏/ });
    expect(sw.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(sw);
    expect(saveAppSettings).toHaveBeenCalledWith({ closeToTray: false });
  });

  it("requires confirmation before enabling auto daily claim", () => {
    const props = renderView();
    fireEvent.click(screen.getByRole("switch", { name: /发现待领福利时自动后台签到/ }));
    expect(saveAppSettings).not.toHaveBeenCalled();
    expect(props.setConfirmLogAction).toHaveBeenCalledTimes(1);
    const action = (props.setConfirmLogAction as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(action.danger).toBe(true);
    return action.run().then(() => {
      expect(saveAppSettings).toHaveBeenCalledWith({ autoClaimDaily: true });
    });
  });

  it("disables auto daily claim immediately without confirmation", () => {
    settings = { ...settings, autoClaimDaily: true };
    const props = renderView();
    fireEvent.click(screen.getByRole("switch", { name: /发现待领福利时自动后台签到/ }));
    expect(props.setConfirmLogAction).not.toHaveBeenCalled();
    expect(saveAppSettings).toHaveBeenCalledWith({ autoClaimDaily: false });
  });

  it("marks path drafts dirty on input and saves them then re-probes clients", async () => {
    const trae = MOCK_ADAPTERS.find((a) => a.capabilities.supportsClientSwitch)!;
    const props = renderView({ pathDrafts: { [trae.id]: "/old" } });
    const input = screen.getByLabelText(`${trae.name} 自定义客户端路径`) as HTMLInputElement;
    expect(input.value).toBe("/old");
    fireEvent.change(input, { target: { value: "/new" } });
    expect(props.setPathDraftsDirty).toHaveBeenCalledWith(true);

    fireEvent.click(screen.getByRole("button", { name: /保存路径覆盖/ }));
    await waitFor(() => expect(refreshClientStatuses).toHaveBeenCalled());
    expect(saveAppSettings).toHaveBeenCalledWith({ customClientPaths: { [trae.id]: "/old" } });
    expect(props.setPathDraftsDirty).toHaveBeenLastCalledWith(false);
  });

  it("keeps drafts dirty when saving paths fails", async () => {
    saveAppSettings.mockResolvedValue(false);
    const props = renderView({ pathDrafts: { a: "/x" } });
    fireEvent.click(screen.getByRole("button", { name: /保存路径覆盖/ }));
    await waitFor(() => expect(saveAppSettings).toHaveBeenCalled());
    expect(props.setPathDraftsDirty).not.toHaveBeenCalled();
    expect(refreshClientStatuses).not.toHaveBeenCalled();
  });

  it("pins content hashes when saving plugin trust and blocks reserved plugins", async () => {
    tauri = true;
    const plugins: ExternalAdapterInfo[] = [
      { id: "ext_ok", name: "Ext OK", version: "1.0.0", platforms: [], trusted: true, reserved: false, contentHash: "h1", hashMismatch: false },
      { id: "trae_cn", name: "Reserved", version: "1.0.0", platforms: [], trusted: false, reserved: true, contentHash: "h2", hashMismatch: false },
    ];
    const props = renderView({ externalPlugins: plugins, trustedDraft: ["ext_ok"] });
    expect((screen.getByRole("switch", { name: /插件 Reserved/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByRole("switch", { name: /停用插件 Ext OK/ }).getAttribute("aria-checked")).toBe("true");

    fireEvent.click(screen.getByRole("button", { name: /保存信任设置/ }));
    await waitFor(() => expect(props.reloadExternalPlugins).toHaveBeenCalled());
    expect(saveAppSettings).toHaveBeenCalledWith(
      { trustedAdapters: ["ext_ok"], trustedAdapterHashes: { ext_ok: "h1" } },
      true
    );
    expect(showToast).toHaveBeenCalledWith("插件信任设置已保存，重启 Qwitch 后生效", "success");
  });
});
