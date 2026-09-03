import { MousePointer2 } from "lucide-react";
import {
  AbsoluteFill,
  Easing,
  Img,
  interpolate,
  spring,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";

import type { SiteTheme } from "../SiteTheme.stylex";

export type MapEveryCopyProps = Record<string, unknown> & {
  theme: SiteTheme;
};

function clamp(frame: number, input: [number, number], output: [number, number]) {
  return interpolate(frame, input, output, {
    easing: Easing.bezier(0.19, 1, 0.22, 1),
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
}

function pulse(frame: number, at: number) {
  const localFrame = frame - at;
  return {
    opacity: interpolate(localFrame, [0, 3, 14], [0, 0.7, 0], {
      extrapolateLeft: "clamp",
      extrapolateRight: "clamp",
    }),
    scale: interpolate(localFrame, [0, 14], [0.45, 1.45], {
      extrapolateLeft: "clamp",
      extrapolateRight: "clamp",
    }),
  };
}

export function MapEveryCopy({ theme }: MapEveryCopyProps) {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const cameraProgress = spring({
    frame,
    fps,
    durationInFrames: 44,
    config: { damping: 200 },
  });
  const cameraScale = interpolate(cameraProgress, [0, 1], [0.75, 0.95]);
  const cameraX = interpolate(cameraProgress, [0, 1], [0, -293]);
  const cameraY = interpolate(cameraProgress, [0, 1], [0, 53]);
  const filteredOpacity = clamp(frame, [88, 96], [0, 1]);
  const cursorX = frame < 65 ? clamp(frame, [8, 40], [1130, 708]) : clamp(frame, [66, 88], [708, 452]);
  const cursorY = frame < 65 ? clamp(frame, [8, 40], [540, 132]) : 132;
  const globalPulse = pulse(frame, 42);
  const searchPulse = pulse(frame, 90);
  const cursorColor = theme === "dark" ? "white" : "#111113";
  const imageBase = `remotion/map-${theme}`;

  return (
    <AbsoluteFill style={{ backgroundColor: theme === "dark" ? "#050506" : "#f6f6f7" }}>
      <div
        style={{
          height: 912,
          left: 0,
          position: "absolute",
          top: 0,
          transform: `translate3d(${cameraX}px, ${cameraY}px, 0) scale(${cameraScale})`,
          transformOrigin: "top left",
          width: 1600,
        }}
      >
        <Img
          src={staticFile(`${imageBase}-global.png`)}
          style={{ height: 912, inset: 0, position: "absolute", width: 1600 }}
        />
        <Img
          src={staticFile(`${imageBase}-filtered.png`)}
          style={{
            height: 912,
            inset: 0,
            opacity: filteredOpacity,
            position: "absolute",
            width: 1600,
          }}
        />
        <div
          style={{
            border: `2px solid ${theme === "dark" ? "#b8a2ff" : "#6f42e8"}`,
            borderRadius: "50%",
            height: 34,
            left: 692,
            opacity: globalPulse.opacity,
            position: "absolute",
            top: 115,
            transform: `scale(${globalPulse.scale})`,
            width: 34,
          }}
        />
        <div
          style={{
            border: `2px solid ${theme === "dark" ? "#b8a2ff" : "#6f42e8"}`,
            borderRadius: "50%",
            height: 34,
            left: 436,
            opacity: searchPulse.opacity,
            position: "absolute",
            top: 115,
            transform: `scale(${searchPulse.scale})`,
            width: 34,
          }}
        />
        <MousePointer2
          fill={cursorColor}
          size={30}
          stroke={theme === "dark" ? "#08080a" : "white"}
          strokeWidth={2}
          style={{
            filter: "drop-shadow(0 2px 3px rgb(0 0 0 / 35%))",
            left: cursorX,
            position: "absolute",
            top: cursorY,
          }}
        />
      </div>
      <AbsoluteFill
        style={{
          boxShadow: `inset 0 0 90px ${theme === "dark" ? "rgb(0 0 0 / 30%)" : "rgb(20 18 26 / 10%)"}`,
          pointerEvents: "none",
        }}
      />
    </AbsoluteFill>
  );
}
