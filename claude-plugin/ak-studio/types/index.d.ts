/** AK Studio の今の状況（帯に出すもの） */
export type StudioView = {
  project: string;
  t: number;
  scene: string;
  clip: string | null;
  lines: number[];
  pending: number;
  engine: string;
  linked: boolean;
};

declare module 'claude-code' {
  interface PluginState {
    'ak-studio': { view: StudioView | null; hidden: boolean };
  }
}
