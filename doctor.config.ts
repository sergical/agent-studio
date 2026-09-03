export default {
  ignore: { files: ["prototypes/**"] },
  // The kit Dialog/Popover from @skill-studio/ui (Base UI) are this app's
  // modal primitives, not the native <dialog> element - see @skill-studio/ui's
  // dialog.tsx and popover.tsx.
  rules: { "react-doctor/prefer-html-dialog": "off" },
};
