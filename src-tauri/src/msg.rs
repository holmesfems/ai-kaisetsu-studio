//! 画面や Claude に出す、何か所からも使う文言。

/// 回（エピソード）を開いていないとき
pub const NO_PROJECT: &str = "回を開いていません";
/// 回に台本（script/）が無いとき
pub const NO_SCRIPT: &str = "台本がありません";
/// 台本の無い回で Claude が台本を読もうとしたとき（作り方の案内つき）
pub const NO_SCRIPT_HINT: &str = "台本がありません（script_apply か script_propose の block_add でブロックを足すと、台本ができます）";
/// 演出のタイムラインがまだ読み込まれていないとき
pub const NO_FX: &str = "演出がまだ読み込まれていません";
