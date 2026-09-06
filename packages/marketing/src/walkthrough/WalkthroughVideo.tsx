import { useEffect, useRef, useState } from "react";
import * as stylex from "@stylexjs/stylex";
import { Maximize2, Pause, Play, X } from "lucide-react";

import { chapters, type WalkthroughProps } from "./walkthrough-chapters";
import { walkthroughStyles as styles } from "./Walkthrough.stylex";

interface WalkthroughVideoProps extends WalkthroughProps {
  animate: boolean;
  compact: boolean;
  index: number;
  onEnded: () => void;
}

export function WalkthroughVideo({
  index,
  theme,
  animate,
  compact,
  onEnded,
}: WalkthroughVideoProps) {
  const chapter = chapters[index];
  const video = useRef<HTMLVideoElement>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const expandButton = useRef<HTMLButtonElement>(null);
  const playbackChoice = useRef<"auto" | "play" | "pause">("auto");
  const [playing, setPlaying] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [failed, setFailed] = useState(false);
  const desktopSource = `/walkthrough/current/${chapter.id}-${theme}-desktop.mp4`;
  const source = `/walkthrough/current/${chapter.id}-${theme}-${compact ? "mobile" : "desktop"}.mp4`;
  const poster = source.replace(/\.mp4$/, ".jpg");

  useEffect(() => {
    const element = video.current;
    if (!element) return;
    const preference = matchMedia("(prefers-reduced-motion: reduce)");
    let visible = false;
    const playback = () => {
      const allowed =
        playbackChoice.current === "play" ||
        (playbackChoice.current === "auto" && !preference.matches);
      if (visible && !document.hidden && allowed && !expanded && !element.ended)
        void element.play().catch(() => undefined);
      else element.pause();
    };
    const observer = new IntersectionObserver(
      ([entry]) => {
        visible = entry.isIntersecting && entry.intersectionRatio >= 0.2;
        playback();
      },
      { threshold: 0.2 },
    );
    observer.observe(element);
    preference.addEventListener("change", playback);
    document.addEventListener("visibilitychange", playback);
    return () => {
      observer.disconnect();
      preference.removeEventListener("change", playback);
      document.removeEventListener("visibilitychange", playback);
      element.pause();
    };
  }, [expanded]);

  useEffect(() => {
    const element = video.current;
    if (!element || !animate) return;
    const transition = element.animate([{ opacity: 0.4 }, { opacity: 1 }], {
      duration: 220,
      easing: "cubic-bezier(0.19, 1, 0.22, 1)",
    });
    return () => transition.cancel();
  }, [animate]);

  useEffect(() => {
    if (!expanded) return;
    dialog.current?.showModal();
    const previous = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = previous;
    };
  }, [expanded]);

  return (
    <div {...stylex.props(styles.media)}>
      <div {...stylex.props(styles.videoFrame)}>
        <video
          ref={video}
          src={source}
          poster={poster}
          preload="none"
          muted
          loop={false}
          playsInline
          aria-label={chapter.title}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onEnded={onEnded}
          onError={() => setFailed(true)}
          {...stylex.props(styles.video, compact && styles.mobileVideo)}
        />
      </div>
      <div {...stylex.props(styles.mediaControls)}>
        {failed && (
          <span role="status" {...stylex.props(styles.mediaError)}>
            This video could not load.
          </span>
        )}
        <button
          aria-label={playing ? "Pause walkthrough" : "Play walkthrough"}
          onClick={() => {
            const element = video.current;
            if (!element) return;
            playbackChoice.current = element.paused ? "play" : "pause";
            if (element.paused) {
              if (element.ended) element.currentTime = 0;
              void element.play().catch(() => setFailed(true));
            } else element.pause();
          }}
          {...stylex.props(styles.mediaButton)}
        >
          {playing ? <Pause size={13} aria-hidden="true" /> : <Play size={13} aria-hidden="true" />}
          {playing ? "Pause" : "Play"}
        </button>
        <button
          ref={expandButton}
          aria-label="Expand walkthrough"
          onClick={() => setExpanded(true)}
          {...stylex.props(styles.mediaButton)}
        >
          <Maximize2 size={13} aria-hidden="true" />
          Expand
        </button>
      </div>
      {expanded && (
        <dialog
          ref={dialog}
          aria-label={chapter.title}
          onClose={() => {
            setExpanded(false);
            expandButton.current?.focus();
          }}
          {...stylex.props(styles.dialog)}
        >
          <div {...stylex.props(styles.dialogHeader)}>
            <h3 {...stylex.props(styles.dialogTitle)}>{chapter.title}</h3>
            <button
              aria-label="Close expanded walkthrough"
              onClick={() => dialog.current?.close()}
              {...stylex.props(styles.mediaButton)}
            >
              <X size={18} />
            </button>
          </div>
          <p {...stylex.props(styles.dialogHint)}>Scroll sideways for a larger view.</p>
          <div {...stylex.props(styles.expandedScroll)}>
            <video
              src={desktopSource}
              poster={desktopSource.replace(/\.mp4$/, ".jpg")}
              controls
              muted
              playsInline
              preload="metadata"
              aria-label={chapter.title}
              {...stylex.props(styles.expandedVideo)}
            />
          </div>
        </dialog>
      )}
    </div>
  );
}
