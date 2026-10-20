import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ChartColumn,
  FlaskConical,
  History,
  Cpu,
  PencilRuler,
  Settings,
} from "lucide-react";
import HandyTextLogo from "./icons/HandyTextLogo";
import ModelSelector from "./model-selector";
import { useSettings } from "../hooks/useSettings";
import { getVersion } from "@tauri-apps/api/app";
import {
  SettingsPage,
  HistorySettings,
  StatsSettings,
  CustomizePage,
  DebugSettings,
  ModelsSettings,
} from "./settings";

export type SidebarSection = keyof typeof SECTIONS_CONFIG;

interface IconProps {
  width?: number | string;
  height?: number | string;
  size?: number | string;
  className?: string;
  [key: string]: any;
}

interface SectionConfig {
  labelKey: string;
  icon: React.ComponentType<IconProps>;
  component: React.ComponentType;
  enabled: (settings: any) => boolean;
  // Secondary sections sit in the bottom group, below a divider.
  secondary?: boolean;
}

export const SECTIONS_CONFIG = {
  history: {
    labelKey: "sidebar.history",
    icon: History,
    component: HistorySettings,
    enabled: () => true,
  },
  stats: {
    labelKey: "sidebar.stats",
    icon: ChartColumn,
    component: StatsSettings,
    enabled: () => true,
  },
  customize: {
    labelKey: "sidebar.customize",
    icon: PencilRuler,
    component: CustomizePage,
    enabled: () => true,
  },
  models: {
    labelKey: "sidebar.models",
    icon: Cpu,
    component: ModelsSettings,
    enabled: () => true,
  },
  debug: {
    labelKey: "sidebar.debug",
    icon: FlaskConical,
    component: DebugSettings,
    enabled: (settings) => settings?.debug_mode ?? false,
    secondary: true,
  },
  settings: {
    labelKey: "sidebar.settings",
    icon: Settings,
    component: SettingsPage,
    enabled: () => true,
    secondary: true,
  },
} as const satisfies Record<string, SectionConfig>;

interface SidebarProps {
  activeSection: SidebarSection;
  onSectionChange: (section: SidebarSection) => void;
}

type AvailableSection = SectionConfig & { id: SidebarSection };

const NavItem: React.FC<{
  section: AvailableSection;
  active: boolean;
  onSelect: (section: SidebarSection) => void;
}> = ({ section, active, onSelect }) => {
  const { t } = useTranslation();
  const Icon = section.icon;
  const label = t(section.labelKey);
  return (
    <button
      type="button"
      className={`flex gap-3 items-center px-3 py-2 w-full rounded-lg cursor-pointer transition-colors text-start ${
        active
          ? "bg-mid-gray/15 text-text"
          : "text-text/75 hover:bg-mid-gray/10 hover:text-text"
      }`}
      onClick={() => onSelect(section.id)}
      aria-current={active ? "page" : undefined}
    >
      <Icon width={20} height={20} strokeWidth={1.75} className="shrink-0" />
      <span className="text-[15px] font-medium truncate" title={label}>
        {label}
      </span>
    </button>
  );
};

export const Sidebar: React.FC<SidebarProps> = ({
  activeSection,
  onSectionChange,
}) => {
  const { settings } = useSettings();
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch((error) => console.error("Failed to get app version:", error));
  }, []);

  const availableSections = Object.entries(SECTIONS_CONFIG)
    .filter(([_, config]) => config.enabled(settings))
    .map(([id, config]) => ({
      id: id as SidebarSection,
      ...(config as SectionConfig),
    }));
  const renderItems = (secondary: boolean) =>
    availableSections
      .filter((section) => (section.secondary ?? false) === secondary)
      .map((section) => (
        <NavItem
          key={section.id}
          section={section}
          active={activeSection === section.id}
          onSelect={onSectionChange}
        />
      ));

  return (
    <nav className="flex flex-col w-52 h-full px-2 pb-3 shrink-0">
      <HandyTextLogo width={96} className="mx-3 mt-4 mb-6" />
      <div className="flex flex-col gap-1">{renderItems(false)}</div>
      <div className="mt-auto flex flex-col gap-1 pt-3 border-t border-mid-gray/15">
        {renderItems(true)}
        <div className="flex flex-col gap-1.5 px-3 pt-3 text-xs text-text/60">
          <ModelSelector />
          {/* eslint-disable-next-line i18next/no-literal-string */}
          {version && <span className="text-text/40">v{version}</span>}
        </div>
      </div>
    </nav>
  );
};
