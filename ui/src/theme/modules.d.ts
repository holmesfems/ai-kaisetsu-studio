// vite.config.ts の alias が指すファイルの型
declare module "@private-ui-css";
declare module "@private-ui-branding" {
  const value: unknown;
  export default value;
}
