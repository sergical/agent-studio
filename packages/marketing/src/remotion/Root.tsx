import { Composition } from "remotion";

import { MapEveryCopy } from "./MapEveryCopy";

export function RemotionRoot() {
  return (
    <>
      <Composition
        id="MapEveryCopyDark"
        component={MapEveryCopy}
        durationInFrames={150}
        fps={30}
        width={1200}
        height={675}
        defaultProps={{ theme: "dark" }}
      />
      <Composition
        id="MapEveryCopyLight"
        component={MapEveryCopy}
        durationInFrames={150}
        fps={30}
        width={1200}
        height={675}
        defaultProps={{ theme: "light" }}
      />
    </>
  );
}
