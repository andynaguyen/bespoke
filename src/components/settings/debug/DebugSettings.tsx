import React from "react";
import { useTranslation } from "react-i18next";
import { WordCorrectionThreshold } from "./WordCorrectionThreshold";
import { LogLevelSelector } from "./LogLevelSelector";
import { LiveLogViewer } from "./LiveLogViewer";
import { PasteDelay } from "./PasteDelay";
import { HoldThreshold } from "./HoldThreshold";
import { ReliablePasteToggle } from "./ReliablePaste";
import { RecordingBuffer } from "./RecordingBuffer";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { AlwaysOnMicrophone } from "../AlwaysOnMicrophone";
import { SoundPicker } from "../SoundPicker";
import { ClamshellMicrophoneSelector } from "../ClamshellMicrophoneSelector";
import { WhatsNewPreview } from "./WhatsNewPreview";
import { KeyboardDiagnostic } from "./KeyboardDiagnostic";
import {
  OnboardingPreview,
  type OnboardingPreviewStep,
} from "./OnboardingPreview";

interface DebugSettingsProps {
  onPreviewOnboarding?: (step: OnboardingPreviewStep) => void;
}

export const DebugSettings: React.FC<DebugSettingsProps> = ({
  onPreviewOnboarding,
}) => {
  const { t } = useTranslation();

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup>
        <LogLevelSelector grouped={true} />
        <WhatsNewPreview descriptionMode="inline" grouped={true} />
        {onPreviewOnboarding && (
          <OnboardingPreview
            onPreview={onPreviewOnboarding}
            descriptionMode="inline"
            grouped={true}
          />
        )}
        <SoundPicker
          label={t("settings.debug.soundTheme.label")}
          description={t("settings.debug.soundTheme.description")}
        />
        <WordCorrectionThreshold descriptionMode="inline" grouped={true} />
        <PasteDelay descriptionMode="inline" grouped={true} />
        <PasteDelay
          descriptionMode="inline"
          grouped={true}
          settingKey="paste_delay_after_ms"
          labelKey="settings.debug.pasteDelayAfter.title"
          descriptionKey="settings.debug.pasteDelayAfter.description"
        />
        <ReliablePasteToggle descriptionMode="inline" grouped={true} />
        <HoldThreshold descriptionMode="inline" grouped={true} />
        <RecordingBuffer descriptionMode="inline" grouped={true} />
        <AlwaysOnMicrophone descriptionMode="inline" grouped={true} />
        <ClamshellMicrophoneSelector descriptionMode="inline" grouped={true} />
        <KeyboardDiagnostic />
        <LiveLogViewer descriptionMode="inline" grouped={true} />
      </SettingsGroup>
    </div>
  );
};
