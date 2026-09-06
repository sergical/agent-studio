import { MousePointer2 } from "lucide-react";
import {
  AbsoluteFill,
  Easing,
  Img,
  interpolate,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";

import type { SiteTheme } from "../SiteTheme.stylex";

export type WalkthroughFeature = "map" | "repair" | "install" | "activity";
export type WalkthroughFormat = "desktop" | "mobile";

export interface ProductWalkthroughProps extends Record<string, unknown> {
  feature: WalkthroughFeature;
  format: WalkthroughFormat;
  theme: SiteTheme;
}

interface CameraKeyframe {
  frame: number;
  centerX: number;
  centerY: number;
  cropWidth: number;
}

interface AssetKeyframe {
  frame: number;
  name: string;
}
interface PointerMove {
  from: number;
  startX: number;
  startY: number;
  to: number;
  x: number;
  y: number;
}
interface StoryProps {
  format: WalkthroughFormat;
  theme: SiteTheme;
}

const ease = Easing.bezier(0.645, 0.045, 0.355, 1);

function cameraValue(
  frame: number,
  keyframes: CameraKeyframe[],
  property: "centerX" | "centerY" | "cropWidth",
) {
  return interpolate(
    frame,
    keyframes.map((keyframe) => keyframe.frame),
    keyframes.map((keyframe) => keyframe[property]),
    {
      easing: keyframes.slice(1).map(() => ease),
      extrapolateLeft: "clamp",
      extrapolateRight: "clamp",
    },
  );
}

function ProductCamera({
  assets,
  camera,
  format,
  pointers = [],
  theme,
}: {
  assets: AssetKeyframe[];
  format: WalkthroughFormat;
  camera: CameraKeyframe[];
  pointers?: PointerMove[];
  theme: SiteTheme;
}) {
  const frame = useCurrentFrame();
  const { height, width } = useVideoConfig();
  const sourceWidth = format === "mobile" ? 800 : 1040;
  const sourceHeight = 1000;
  const cropWidth = cameraValue(frame, camera, "cropWidth");
  const cropHeight = (cropWidth * height) / width;
  const centerX = Math.min(
    sourceWidth - cropWidth / 2,
    Math.max(cropWidth / 2, cameraValue(frame, camera, "centerX")),
  );
  const centerY = Math.min(
    sourceHeight - cropHeight / 2,
    Math.max(cropHeight / 2, cameraValue(frame, camera, "centerY")),
  );
  const scale = width / cropWidth;
  const left = width / 2 - centerX * scale;
  const top = height / 2 - centerY * scale;
  let activeAsset = 0;
  for (let index = assets.length - 1; index >= 0; index -= 1) {
    if (frame >= assets[index].frame) {
      activeAsset = index;
      break;
    }
  }
  const nextAssetFrame = assets[activeAsset + 1]?.frame;
  const crossfade =
    nextAssetFrame === undefined
      ? 0
      : interpolate(frame, [nextAssetFrame - 3, nextAssetFrame + 2], [0, 1], {
          easing: Easing.bezier(0.19, 1, 0.22, 1),
          extrapolateLeft: "clamp",
          extrapolateRight: "clamp",
        });

  return (
    <AbsoluteFill
      style={{ backgroundColor: theme === "dark" ? "#09090b" : "#fafafa", overflow: "hidden" }}
    >
      <div
        style={{
          height: sourceHeight,
          left,
          position: "absolute",
          scale,
          top,
          transformOrigin: "top left",
          width: sourceWidth,
        }}
      >
        {assets.map((asset, index) => (
          <Img
            key={asset.frame}
            name={asset.name}
            src={staticFile(
              `remotion/current/${asset.name}${format === "mobile" ? "-mobile" : ""}.png`,
            )}
            style={{
              height: sourceHeight,
              inset: 0,
              opacity:
                index === activeAsset ? 1 - crossfade : index === activeAsset + 1 ? crossfade : 0,
              position: "absolute",
              width: sourceWidth,
            }}
          />
        ))}
        {pointers.map((pointer) => {
          const travel = interpolate(frame, [pointer.from, pointer.to], [0, 1], {
            easing: Easing.bezier(0.19, 1, 0.22, 1),
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
          });
          return (
            <div
              key={`${pointer.from}-${pointer.to}`}
              style={{
                filter: "drop-shadow(0 2px 3px rgb(0 0 0 / 45%))",
                left: interpolate(travel, [0, 1], [pointer.startX, pointer.x]),
                opacity: interpolate(
                  frame,
                  [pointer.from - 6, pointer.from, pointer.to + 7, pointer.to + 13],
                  [0, 1, 1, 0],
                  {
                    extrapolateLeft: "clamp",
                    extrapolateRight: "clamp",
                  },
                ),
                position: "absolute",
                scale:
                  interpolate(
                    frame,
                    [pointer.to - 2, pointer.to + 2, pointer.to + 7],
                    [1, 0.92, 1],
                    {
                      easing: [
                        Easing.bezier(0.25, 0.46, 0.45, 0.94),
                        Easing.bezier(0.19, 1, 0.22, 1),
                      ],
                      extrapolateLeft: "clamp",
                      extrapolateRight: "clamp",
                      output: "perceptual-scale",
                    },
                  ) / scale,
                top: interpolate(travel, [0, 1], [pointer.startY, pointer.y]),
                transformOrigin: "top left",
              }}
            >
              <MousePointer2
                fill={theme === "dark" ? "#fff" : "#111113"}
                size={30}
                stroke={theme === "dark" ? "#111113" : "#fff"}
                strokeWidth={2}
              />
            </div>
          );
        })}
      </div>
    </AbsoluteFill>
  );
}

function MapStory({ format, theme }: StoryProps) {
  const mobile = format === "mobile";
  return (
    <ProductCamera
      format={format}
      theme={theme}
      assets={[
        { frame: 0, name: `map-${theme}-global` },
        { frame: 120, name: `map-${theme}-expanded` },
        { frame: 264, name: `map-${theme}-compare` },
      ]}
      camera={
        mobile
          ? [
              { frame: 0, centerX: 520, centerY: 280, cropWidth: 560 },
              { frame: 24, centerX: 520, centerY: 280, cropWidth: 560 },
              { frame: 64, centerX: 520, centerY: 440, cropWidth: 500 },
              { frame: 119, centerX: 520, centerY: 440, cropWidth: 500 },
              { frame: 160, centerX: 520, centerY: 665, cropWidth: 500 },
              { frame: 205, centerX: 520, centerY: 665, cropWidth: 500 },
              { frame: 240, centerX: 520, centerY: 330, cropWidth: 500 },
              { frame: 263, centerX: 520, centerY: 330, cropWidth: 500 },
              { frame: 264, centerX: 400, centerY: 500, cropWidth: 800 },
              { frame: 290, centerX: 400, centerY: 500, cropWidth: 800 },
              { frame: 315, centerX: 225, centerY: 525, cropWidth: 440 },
              { frame: 355, centerX: 225, centerY: 525, cropWidth: 440 },
              { frame: 390, centerX: 575, centerY: 545, cropWidth: 440 },
            ]
          : [
              { frame: 0, centerX: 640, centerY: 265, cropWidth: 900 },
              { frame: 24, centerX: 640, centerY: 265, cropWidth: 900 },
              { frame: 64, centerX: 640, centerY: 420, cropWidth: 800 },
              { frame: 119, centerX: 640, centerY: 420, cropWidth: 800 },
              { frame: 160, centerX: 640, centerY: 600, cropWidth: 800 },
              { frame: 205, centerX: 640, centerY: 600, cropWidth: 800 },
              { frame: 240, centerX: 640, centerY: 300, cropWidth: 800 },
              { frame: 263, centerX: 640, centerY: 300, cropWidth: 800 },
              { frame: 264, centerX: 520, centerY: 500, cropWidth: 1000 },
              { frame: 290, centerX: 520, centerY: 500, cropWidth: 1000 },
              { frame: 325, centerX: 520, centerY: 530, cropWidth: 940 },
            ]
      }
      pointers={[
        {
          from: 91,
          startX: 540,
          startY: mobile ? 640 : 600,
          to: 115,
          x: 299,
          y: mobile ? 572 : 534,
        },
        {
          from: 234,
          startX: mobile ? 570 : 780,
          startY: 330,
          to: 259,
          x: mobile ? 693 : 943,
          y: mobile ? 231 : 193,
        },
      ]}
    />
  );
}

function RepairStory({ format, theme }: StoryProps) {
  const mobile = format === "mobile";
  return (
    <ProductCamera
      format={format}
      theme={theme}
      assets={[
        { frame: 0, name: `repair-${theme}-broken` },
        { frame: 155, name: `repair-${theme}-resolved` },
      ]}
      camera={
        mobile
          ? [
              { frame: 0, centerX: 520, centerY: 685, cropWidth: 520 },
              { frame: 35, centerX: 520, centerY: 685, cropWidth: 520 },
              { frame: 62, centerX: 520, centerY: 650, cropWidth: 500 },
              { frame: 105, centerX: 520, centerY: 650, cropWidth: 500 },
              { frame: 135, centerX: 520, centerY: 750, cropWidth: 500 },
              { frame: 154, centerX: 520, centerY: 750, cropWidth: 500 },
              { frame: 155, centerX: 520, centerY: 495, cropWidth: 520 },
            ]
          : [
              { frame: 0, centerX: 640, centerY: 665, cropWidth: 800 },
              { frame: 35, centerX: 640, centerY: 665, cropWidth: 800 },
              { frame: 62, centerX: 640, centerY: 610, cropWidth: 740 },
              { frame: 105, centerX: 640, centerY: 610, cropWidth: 740 },
              { frame: 135, centerX: 640, centerY: 710, cropWidth: 740 },
              { frame: 154, centerX: 640, centerY: 710, cropWidth: 740 },
              { frame: 155, centerX: 640, centerY: 485, cropWidth: 800 },
            ]
      }
      pointers={[{ from: 121, startX: 475, startY: 790, to: 150, x: 326, y: mobile ? 864 : 846 }]}
    />
  );
}

function InstallStory({ format, theme }: StoryProps) {
  const mobile = format === "mobile";
  const sheetX = mobile ? 590 : 830;
  return (
    <ProductCamera
      format={format}
      theme={theme}
      assets={[
        { frame: 0, name: `install-${theme}-project` },
        { frame: 110, name: `install-${theme}-trial` },
        { frame: 195, name: `install-${theme}-installed` },
      ]}
      camera={
        mobile
          ? [
              { frame: 0, centerX: sheetX, centerY: 300, cropWidth: 420 },
              { frame: 30, centerX: sheetX, centerY: 300, cropWidth: 420 },
              { frame: 70, centerX: sheetX, centerY: 630, cropWidth: 420 },
              { frame: 145, centerX: sheetX, centerY: 630, cropWidth: 420 },
              { frame: 175, centerX: sheetX, centerY: 800, cropWidth: 420 },
              { frame: 194, centerX: sheetX, centerY: 800, cropWidth: 420 },
              { frame: 195, centerX: 520, centerY: 250, cropWidth: 520 },
            ]
          : [
              { frame: 0, centerX: 690, centerY: 300, cropWidth: 700 },
              { frame: 30, centerX: 690, centerY: 300, cropWidth: 700 },
              { frame: 70, centerX: 710, centerY: 635, cropWidth: 660 },
              { frame: 145, centerX: 710, centerY: 635, cropWidth: 660 },
              { frame: 175, centerX: 710, centerY: 820, cropWidth: 660 },
              { frame: 194, centerX: 710, centerY: 820, cropWidth: 660 },
              { frame: 195, centerX: 640, centerY: 245, cropWidth: 800 },
            ]
      }
      pointers={[
        { from: 80, startX: sheetX - 60, startY: 595, to: 105, x: sheetX - 181, y: 659 },
        { from: 160, startX: sheetX - 20, startY: 875, to: 190, x: sheetX + 147, y: 968 },
      ]}
    />
  );
}

function ActivityStory({ format, theme }: StoryProps) {
  const mobile = format === "mobile";
  return (
    <ProductCamera
      format={format}
      theme={theme}
      assets={[
        { frame: 0, name: `activity-${theme}-30d` },
        { frame: 100, name: `activity-${theme}-7d` },
      ]}
      camera={
        mobile
          ? [
              { frame: 0, centerX: 520, centerY: 280, cropWidth: 560 },
              { frame: 52, centerX: 520, centerY: 310, cropWidth: 520 },
              { frame: 150, centerX: 520, centerY: 310, cropWidth: 520 },
              { frame: 190, centerX: 520, centerY: 475, cropWidth: 520 },
            ]
          : [
              { frame: 0, centerX: 640, centerY: 235, cropWidth: 800 },
              { frame: 52, centerX: 640, centerY: 320, cropWidth: 800 },
              { frame: 150, centerX: 640, centerY: 320, cropWidth: 800 },
              { frame: 190, centerX: 640, centerY: 445, cropWidth: 800 },
            ]
      }
      pointers={[
        {
          from: 66,
          startX: mobile ? 600 : 800,
          startY: 400,
          to: 95,
          x: mobile ? 666 : 906,
          y: mobile ? 250 : 282,
        },
      ]}
    />
  );
}

export function ProductWalkthrough({ feature, format, theme }: ProductWalkthroughProps) {
  if (feature === "map") return <MapStory format={format} theme={theme} />;
  if (feature === "repair") return <RepairStory format={format} theme={theme} />;
  if (feature === "install") return <InstallStory format={format} theme={theme} />;
  return <ActivityStory format={format} theme={theme} />;
}
