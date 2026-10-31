// 帯の文言。上書き（presets/ui/branding.json）が見つからなければ none.json が入る（vite.config.ts の alias）
// 上書きの CSS（@private-ui-css）は、ここではなく main.tsx で styles.css の後に import する（読む順が値の優先順になる）
import override from "@private-ui-branding";
import { mergeBranding } from "../logic/branding.ts";

export const branding = mergeBranding(override);
