#!/usr/bin/env python3
"""Source of packs/item-dictionary/items.json (run this, then validate with
scripts/packs/validate-dictionary.py).

One line per item in ROWS:  category | subcategory | name_ja | kana | name_en |
synonyms (;) | units (;) | typical packs (;) | spec attributes (;) | flags
  kana    - reading (hiragana/katakana); empty = name_ja is already kana
  flags   - "u" = uncertain (pack sizes or naming vary a lot; check with the buyer)
Part-number patterns are in MPN, keyed by name_en. Only JIS/ISO/IEC designation formats and
well-known, published manufacturer series formats are listed; nothing is invented.
Written by hand for Chuo-Sogo Procure; units and pack sizes are typical, not exhaustive.
"""
import json
import pathlib
import re

ROWS = """
# ---------------- electrical: wire and cable ----------------
electrical|cable|VVFケーブル|ぶいぶいえふけーぶる|VVF flat cable (600V PVC sheathed)|VVF;Fケーブル;VA線;VAケーブル;ビニル絶縁ビニルシースケーブル平形;電線;ケーブル;エフケーブル|巻;m|100m巻;切り売り|conductor_mm(1.6/2.0/2.6);cores(2C/3C);earth_core|
electrical|cable|VVRケーブル|ぶいぶいあーるけーぶる|VVR round cable (600V PVC sheathed)|VVR;ビニル絶縁ビニルシースケーブル丸形;SV|m;巻|切り売り;100m巻|size_sq;cores|
electrical|cable|CVケーブル|しーぶいけーぶる|CV cable (XLPE insulated PVC sheathed)|CV;架橋ポリエチレン絶縁ビニルシースケーブル;動力ケーブル;CV線|m;巻|切り売り;100m巻|size_sq;cores(1C/2C/3C/4C);voltage(600V)|
electrical|cable|CVTケーブル|しーぶいてぃーけーぶる|CVT triplex cable|CVT;トリプレックス形CVケーブル;CV-T|m|切り売り|size_sq;voltage|
electrical|cable|EM-EEFケーブル|えむいーいーえふけーぶる|EM-EEF eco flat cable (halogen-free)|EM-EEF;エコケーブル;EEF;EM-EEF/F;耐燃性ポリエチレンシースケーブル平形|巻;m|100m巻|conductor_mm;cores|
electrical|cable|EM-CEケーブル|えむしーいーけーぶる|EM-CE eco power cable|EM-CE;エコCV;EM-CE/F|m|切り売り|size_sq;cores|
electrical|wire|IV電線|あいぶいでんせん|IV wire (600V PVC insulated)|IV;IV線;ビニル絶縁電線;単線;より線|巻;m|300m巻;切り売り|conductor(1.6mm/2.0mm/5.5sq/8sq);colour|
electrical|wire|HIV電線|えいちあいぶいでんせん|HIV heat-resistant PVC wire|HIV;二種ビニル絶縁電線;耐熱電線|巻;m|300m巻;切り売り|conductor;colour|
electrical|wire|KIV電線|けーあいぶいでんせん|KIV equipment wire (panel wiring)|KIV;電気機器用ビニル絶縁電線;盤内配線用電線;KIV線|巻;m|100m巻;200m巻;300m巻|size_sq(0.75-8);colour|u
electrical|wire|EM-IE電線|えむあいいーでんせん|EM-IE eco insulated wire|EM-IE;エコIV;EM-IE/F|巻;m|300m巻|conductor;colour|
electrical|wire|アース線|あーすせん|Earth wire (green IV)|接地線;緑線;IV緑;グリーン線;E線|巻;m|300m巻;切り売り|conductor;colour(green)|
electrical|wire|UL1007電線|ゆーえるいちぜろぜろななでんせん|UL1007 hook-up wire|UL1007;AWG電線;UL線;機器配線用電線|巻|305m巻;100m巻|awg;colour|u
electrical|wire|UL1015電線|ゆーえるいちぜろいちごでんせん|UL1015 hook-up wire (600V)|UL1015;UL線|巻|305m巻;100m巻|awg;colour|u
electrical|cord|VCTFコード|ぶいしーてぃーえふこーど|VCTF round PVC cabtyre cord|VCTF;ビニルキャブタイヤ丸形コード;キャブタイヤコード;キャプタイヤ|巻;m|100m巻;切り売り|size_sq(0.3-2.0);cores|
electrical|cord|VCTケーブル|ぶいしーてぃーけーぶる|VCT PVC cabtyre cable|VCT;ビニルキャブタイヤケーブル;キャブタイヤケーブル|巻;m|100m巻;切り売り|size_sq;cores|
electrical|cord|2PNCTケーブル|にぴーえぬしーてぃーけーぶる|2PNCT rubber cabtyre cable|2PNCT;PNCT;ゴムキャブタイヤケーブル;2種クロロプレンキャブタイヤ|巻;m|100m巻;切り売り|size_sq;cores|
electrical|cord|VFFコード|ぶいえふえふこーど|VFF flat parallel cord|VFF;平形ビニルコード;袋打ちコード;並行コード|巻|100m巻|size_sq|
electrical|cable|CVVケーブル|しーぶいぶいけーぶる|CVV control cable|CVV;制御用ビニル絶縁ビニルシースケーブル;制御ケーブル|m;巻|切り売り;100m巻|size_sq(1.25/2.0);cores(2C-30C)|
electrical|cable|CVV-Sケーブル|しーぶいぶいえすけーぶる|CVV-S shielded control cable|CVV-S;シールド付制御ケーブル;遮へい付制御ケーブル|m;巻|切り売り;100m巻|size_sq;cores|
electrical|cable|計装用ケーブル|けいそうようけーぶる|Instrumentation cable (shielded twisted pair)|計装ケーブル;KPEV-S;KPEV;ツイストペアシールド|m;巻|切り売り;200m巻|pairs;size_sq;shield|u
electrical|cable|熱電対補償導線|ねつでんついほしょうどうせん|Thermocouple extension/compensating cable|補償導線;Kタイプ補償導線;KX;熱電対ケーブル|m;巻|切り売り;100m巻|type(K/J/T);cores;insulation|
electrical|cable|ロボットケーブル|ろぼっとけーぶる|Flexible robot / drag-chain cable|耐屈曲ケーブル;可動部用ケーブル;ケーブルベア用ケーブル|m|切り売り|size_sq;cores;bend_radius|u
electrical|cable|LANケーブル Cat6|らんけーぶる|LAN cable Cat6|UTPケーブル;Cat6;カテ6;イーサネットケーブル;ツイストペアケーブル|本;巻|1m;3m;5m;10m;300m巻|category;length;shield(UTP/STP)|
electrical|cable|LANケーブル Cat5e|らんけーぶる|LAN cable Cat5e|Cat5e;カテ5e;UTPケーブル|本;巻|1m;3m;5m;300m巻|category;length|
electrical|cable|同軸ケーブル|どうじくけーぶる|Coaxial cable|5C-FB;3C-2V;5C-2V;同軸|m;巻|切り売り;100m巻|impedance(75Ω/50Ω);type|
electrical|cable|耐火ケーブル|たいかけーぶる|Fire-resistant cable|FPケーブル;耐火電線;HP;耐熱ケーブル|m;巻|切り売り;100m巻|size;cores;rating|u
electrical|cable|MVVSケーブル|えむぶいぶいえすけーぶる|MVVS shielded microphone/signal cable|MVVS;マイクロホン用ビニルコード;シールド線|m;巻|100m巻|size_sq;cores|
electrical|cable|フラットケーブル|ふらっとけーぶる|Ribbon (flat) cable|リボンケーブル;FFC|m;巻|30m巻|pitch(1.27mm);cores|u
# ---------------- electrical: conduit, raceway, support ----------------
electrical|conduit|PF管|ぴーえふかん|PF flexible conduit (self-extinguishing)|PF-S;合成樹脂製可とう電線管;フレキ;PF-D|巻|50m巻;30m巻|size(14/16/22/28);single/double_wall|
electrical|conduit|CD管|しーでぃーかん|CD flexible conduit (for embedding in concrete)|CD;オレンジ管;コンクリート埋設用可とう管|巻|50m巻|size(14/16/22/28)|
electrical|conduit|VE管|ぶいいーかん|VE rigid PVC conduit|VE;硬質ビニル電線管;塩ビ電線管|本|4m|size(VE14/16/22/28/36)|
electrical|conduit|E管|いーかん|E threadless steel conduit|ねじなし電線管;E管;EG管|本|3.66m|size(E19/E25/E31)|
electrical|conduit|C管|しーかん|C thin steel conduit|薄鋼電線管;C管|本|3.66m|size(C19/C25/C31)|
electrical|conduit|G管|じーかん|G thick steel conduit|厚鋼電線管;G管;ガス管(俗)|本|3.66m|size(G16/G22/G28)|
electrical|conduit|FEP管|えふいーぴーかん|FEP corrugated conduit (underground)|波付硬質合成樹脂管;FEP;地中埋設管|巻;m|50m巻;30m巻|size(30/40/50/65/80)|
electrical|conduit|プリカチューブ|ぷりかちゅーぶ|Flexible metal conduit (Plica type)|金属製可とう電線管;プリカ;2種金属製可とう電線管;フレキシブルコンジット|巻;m|50m巻;切り売り|size(10-101);waterproof_type|
electrical|conduit|金属製可とう電線管用コネクタ|きんぞくせいかとうでんせんかんようこねくた|Flexible conduit connector|プリカコネクタ;ボックスコネクタ(プリカ用)|個|1個;10個入|size;thread|
electrical|conduit|電線管カップリング|でんせんかんかっぷりんぐ|Conduit coupling|カップリング;ねじなしカップリング;PFカップリング|個|1個;10個入|conduit_type;size|
electrical|conduit|ボックスコネクタ|ぼっくすこねくた|Box connector|ねじなしボックスコネクタ;PFボックスコネクタ|個|1個;10個入|conduit_type;size|
electrical|conduit|電線管サドル|でんせんかんさどる|Conduit saddle|サドル;片サドル;両サドル;PFサドル|個;袋|10個入;100個入|conduit_size|
electrical|conduit|ステップル|すてっぷる|Cable staple (for VVF)|ステープル;VVFステップル;絶縁ステップル|箱;袋|100個入;1000個入|cable_size|
electrical|conduit|エントランスキャップ|えんとらんすきゃっぷ|Entrance cap|エンドキャップ;雨よけキャップ|個|1個|conduit_size|
electrical|conduit|ブッシング|ぶっしんぐ|Conduit bushing|絶縁ブッシング;ゴムブッシング;グロメット|個;袋|1個;10個入|size|
electrical|conduit|ケーブルグランド|けーぶるぐらんど|Cable gland (waterproof)|防水グランド;防水コネクタ;グランド;ケーブルクランプ(防水)|個|1個;10個入|thread(PG/M/G);cable_dia;ip_rating|
electrical|raceway|配線ダクト|はいせんだくと|Wiring duct (slotted, panel)|スリットダクト;配線ダクト;ダクト;ワイヤリングダクト|本|2m;1m|width;height;colour(grey)|
electrical|raceway|配線モール|はいせんもーる|Surface wire molding|モール;ワイヤープロテクター;ケーブルカバー;床モール|本|1m;2m|size;colour|
electrical|raceway|ケーブルラック|けーぶるらっく|Cable tray (ladder rack)|ラック;ケーブルトレイ|本|3m;4m|width;material(steel/aluminium/SUS)|u
electrical|raceway|レースウェイ|れーすうぇい|Raceway (steel channel)|ダクター;Dチャン;ダクターチャンネル|本|2.5m;4m|size;finish|u
electrical|raceway|DINレール|でぃんれーる|DIN rail (35mm)|DINレール;TH35;取付レール|本|1m;2m|width(35mm);height(7.5/15)|
electrical|raceway|エンドプレート|えんどぷれーと|DIN rail end stopper|エンドストッパ;エンドクランプ|個;袋|10個入|rail_type|
electrical|raceway|スパイラルチューブ|すぱいらるちゅーぶ|Spiral wrapping tube|スパイラル;結束チューブ|巻|10m巻;30m巻|bundle_dia;colour|
electrical|raceway|コルゲートチューブ|こるげーとちゅーぶ|Corrugated split tube|コルゲート;ジャバラチューブ|巻;m|10m巻;50m巻|inner_dia;split|
electrical|raceway|ケーブルクランプ|けーぶるくらんぷ|Cable clamp (P-clip)|Pクランプ;コードクリップ;ケーブルクリップ|袋;個|10個入;100個入|cable_dia;material|
electrical|raceway|結束バンド|けっそくばんど|Cable tie|インシュロック;タイラップ;ケーブルタイ;ナイロンバンド|袋|100本入;1000本入|length_mm;width_mm;colour;weather_resistant|
electrical|raceway|結束バンド固定具|けっそくばんどこていぐ|Cable tie mount|マウントベース;タイマウント|袋|100個入|tie_width;adhesive/screw|
electrical|raceway|プルボックス|ぷるぼっくす|Pull box|プルボックス;中継ボックス|個|1個|size;material;outdoor|
electrical|raceway|アウトレットボックス|あうとれっとぼっくす|Outlet box|アウトレットボックス;中四角;大四角|個|1個;10個入|size;depth;knockouts|
electrical|raceway|スイッチボックス|すいっちぼっくす|Switch box|スイッチボックス;1個用;2個用;埋込ボックス|個|1個;10個入|gang;depth|
electrical|raceway|ジョイントボックス|じょいんとぼっくす|Junction box|ジャンクションボックス;中継ボックス;JB|個|1個|size;ip_rating|
electrical|raceway|防水ボックス|ぼうすいぼっくす|Waterproof enclosure box|プラボックス;防水プラボックス;防雨ボックス|個|1個|size;ip_rating;material|
# ---------------- electrical: terminals and connection ----------------
electrical|terminal|圧着端子 R形|あっちゃくたんし|Ring crimp terminal (R type)|丸端子;R形端子;裸圧着端子;R端子;リング端子|袋;箱|100個入;1000個入|wire_size(1.25/2/5.5/8sq);stud(M3-M10);insulated|
electrical|terminal|圧着端子 Y形|あっちゃくたんし|Fork crimp terminal (Y type)|先開形端子;Y端子;フォーク端子|袋;箱|100個入;1000個入|wire_size;stud|
electrical|terminal|棒端子|ぼうたんし|Ferrule / pin terminal|フェルール;スリーブ端子;絶縁被覆付棒端子;ピン端子|袋;箱|100個入;500個入|wire_size;length;colour|
electrical|terminal|閉端接続子|へいたんせつぞくし|Closed-end splice|CE;CE1;CE2;CE5;閉端子|袋|100個入|size(CE1/CE2/CE5/CE8)|
electrical|terminal|リングスリーブ|りんぐすりーぶ|Ring sleeve (E-type crimp)|E形スリーブ;小スリーブ;中スリーブ;Eスリーブ|袋|100個入|size(小/中/大)|
electrical|terminal|差込形電線コネクタ|さしこみがたでんせんこねくた|Push-in wire connector|差込コネクタ;ワゴ;WAGO;クイックロック;ワンタッチコネクタ|袋;箱|50個入;100個入|poles(2-8);wire_size|
electrical|terminal|絶縁キャップ|ぜつえんきゃっぷ|Insulation cap for terminals|端子カバー;絶縁カバー;ビニルキャップ|袋|100個入|size;colour|
electrical|terminal|突合せスリーブ|つきあわせすりーぶ|Butt splice|B形スリーブ;突合せ端子;中継スリーブ|袋|100個入|wire_size;insulated|
electrical|terminal|端子台|たんしだい|Terminal block (screw, panel)|ターミナルブロック;端子板;ネジ端子台|個|1個;10個入|poles;current_A;screw(M3.5/M4)|
electrical|terminal|DIN端子台|でぃんたんしだい|DIN-rail terminal block|レール取付端子台;組端子台;スプリング端子台;プッシュイン端子台|個;箱|1個;50個入|wire_size;screw/spring;colour|
electrical|terminal|マーカーチューブ|まーかーちゅーぶ|Wire marker tube|マークチューブ;チューブマーカー;印字チューブ|巻|100m巻|inner_dia;colour|u
electrical|terminal|ケーブルマーカー|けーぶるまーかー|Cable marker (clip-on)|電線マーカー;記号札;ケーブルタグ|袋|100個入|character;size|
electrical|terminal|熱収縮チューブ|ねつしゅうしゅくちゅーぶ|Heat-shrink tube|シュリンクチューブ;スミチューブ;収縮チューブ|本;巻|1m;10本入;巻|inner_dia;shrink_ratio;colour|
electrical|terminal|丸形コネクタ|まるがたこねくた|Circular connector (industrial)|丸型コネクタ;防水コネクタ;M12コネクタ;NCS|個|1個|pins;thread;ip_rating|u
electrical|terminal|圧着コネクタハウジング|あっちゃくこねくたはうじんぐ|Wire-to-board connector housing|ハウジング;コネクタハウジング;XH;PH|袋|100個入;1000個入|pitch;pins|u
# ---------------- electrical: tapes ----------------
electrical|tape|ビニルテープ|びにるてーぷ|PVC electrical insulating tape|ビニテ;絶縁テープ;電気用ビニルテープ|巻;パック|1巻;10巻パック|width(19mm);length(10m/20m);colour|
electrical|tape|自己融着テープ|じこゆうちゃくてーぷ|Self-amalgamating tape|融着テープ;ブチルゴムテープ;防水テープ(電気)|巻|1巻|width;thickness;length|
# ---------------- electrical: protection and distribution ----------------
electrical|protection|配線用遮断器|はいせんようしゃだんき|Moulded-case circuit breaker (MCCB)|MCCB;ブレーカ;ブレーカー;ノーヒューズブレーカー;NFB;ノーヒューズ;遮断器|台;個|1台|poles(2P/3P);frame_AF;rated_AT;breaking_kA|
electrical|protection|漏電遮断器|ろうでんしゃだんき|Earth-leakage circuit breaker (ELCB)|ELCB;漏電ブレーカ;漏電ブレーカー;ELB;RCD|台;個|1台|poles;frame_AF;rated_AT;sensitivity_mA(15/30)|
electrical|protection|分岐ブレーカ|ぶんきぶれーか|Branch breaker (distribution board)|分岐ブレーカー;子ブレーカ;安全ブレーカ;小形ブレーカ|個|1個|poles;rated_A;plug-in/bolt-on|
electrical|protection|サーキットプロテクタ|さーきっとぷろてくた|Circuit protector (miniature, control circuit)|CP;サーキットプロテクター;CP30|個|1個|poles;rated_A;trip_curve|
electrical|protection|筒形ヒューズ|つつがたひゅーず|Cartridge fuse|ヒューズ;シリンドリカルヒューズ;栓形ヒューズ|個;箱|1個;10個入|size;rated_A;rated_V|
electrical|protection|ガラス管ヒューズ|がらすかんひゅーず|Glass tube fuse|管ヒューズ;ミニヒューズ;速断ヒューズ;タイムラグヒューズ|箱;袋|10本入|size(5x20/6.4x30);rated_A|
electrical|protection|ヒューズホルダ|ひゅーずほるだ|Fuse holder|ヒューズホルダー;ヒューズ台|個|1個|fuse_size;poles|
electrical|protection|サージ保護デバイス|さーじほごでばいす|Surge protective device (SPD)|SPD;避雷器;サージアブソーバ;アレスタ|個|1個|voltage;class;poles|
electrical|protection|ノイズフィルタ|のいずふぃるた|EMI / noise filter|ノイズフィルター;ラインフィルタ;EMIフィルタ|個|1個|rated_A;phase;voltage|
electrical|protection|フェライトコア|ふぇらいとこあ|Ferrite core (clamp)|ノイズ対策コア;クランプフィルタ|個|1個;10個入|cable_dia|
electrical|protection|漏電リレー|ろうでんりれー|Earth-leakage relay|ELR;漏電継電器|台|1台|sensitivity;zct|u
electrical|distribution|分電盤|ぶんでんばん|Distribution board|ブレーカー盤;電灯分電盤;動力分電盤|面|1面|main_breaker;branches;phase|u
electrical|distribution|制御盤用キャビネット|せいぎょばんようきゃびねっと|Control panel enclosure|制御盤;盤;キャビネット;ボックス(盤用);プルボックス(盤)|台;面|1台|size_whd;material;ip_rating|
electrical|distribution|盤用ファン|ばんようふぁん|Panel cooling fan with filter|フィルタファン;盤用換気扇;換気ファン(盤)|台|1台|size;voltage;airflow|
electrical|distribution|盤用フィルタ|ばんようふぃるた|Replacement filter mat for panel fan|フィルタマット;ファンフィルタ|袋;枚|5枚入|size|
electrical|distribution|盤用照明|ばんようしょうめい|Panel LED light|盤内灯;キャビネットライト|台|1台|length;voltage|u
electrical|distribution|制御用変圧器|せいぎょようへんあつき|Control transformer|トランス;変圧器;単巻トランス;絶縁トランス;ダウントランス|台|1台|capacity_VA;primary_V;secondary_V|
electrical|distribution|変流器|へんりゅうき|Current transformer (CT)|CT;カレントトランス;クランプCT|台;個|1個|ratio;burden_VA;class|
electrical|distribution|零相変流器|れいそうへんりゅうき|Zero-phase current transformer (ZCT)|ZCT|個|1個|hole_dia|u
# ---------------- electrical: control components ----------------
electrical|control|電磁接触器|でんじせっしょくき|Magnetic contactor|MC;マグネット;コンタクタ;電磁コンタクタ;マグネットコンタクタ|台;個|1台|frame;coil_V;aux_contacts;kW_rating|
electrical|control|電磁開閉器|でんじかいへいき|Magnetic starter (contactor + overload)|MS;マグネットスイッチ;電磁スイッチ|台|1台|frame;coil_V;heater_A;enclosure|
electrical|control|サーマルリレー|さーまるりれー|Thermal overload relay|熱動形過負荷継電器;THR;サーマル;過負荷継電器|台;個|1個|heater_A_range;frame|
electrical|control|補助接点ユニット|ほじょせってんゆにっと|Auxiliary contact block|補助接点;補助接点ブロック;UN-AX|個|1個|contacts(1a1b/2a2b)|
electrical|control|ミニパワーリレー|みにぱわーりれー|General-purpose plug-in relay|リレー;継電器;制御リレー;MYリレー;ミニリレー;パワーリレー|個;箱|1個;10個入|coil_V(DC24/AC100);contacts(2c/4c);socket|
electrical|control|リレーソケット|りれーそけっと|Relay socket|ソケット(リレー);共用ソケット;DINソケット;PYF|個;箱|1個;10個入|pins;mount|
electrical|control|ソリッドステートリレー|そりっどすてーとりれー|Solid-state relay (SSR)|SSR;半導体リレー;無接点リレー|個|1個|load_A;load_V;input_V|
electrical|control|インターフェースリレー|いんたーふぇーすりれー|Slim interface relay (I/O)|I/Oリレー;スリムリレー;G2RV|個;箱|1個;10個入|coil_V;width|u
electrical|control|タイマー|たいまー|Timer relay|タイマ;限時継電器;タイムリレー;オンディレータイマ|個|1個|range;mode;voltage;socket|
electrical|control|カウンタ|かうんた|Counter (preset)|カウンター;プリセットカウンタ;トータルカウンタ|個|1個|digits;input;voltage|
electrical|control|押しボタンスイッチ|おしぼたんすいっち|Push-button switch|押釦;押しボタン;PB;プッシュボタン;押釦スイッチ|個|1個|dia(16/22/25/30);colour;contacts;illuminated|
electrical|control|セレクタスイッチ|せれくたすいっち|Selector switch|切替スイッチ;セレクター;SS;カムスイッチ|個|1個|positions;dia;contacts|
electrical|control|非常停止スイッチ|ひじょうていしすいっち|Emergency stop push-button|非常停止ボタン;非停;E-STOP;非常停止押しボタン;キノコボタン|個|1個|dia;contacts(NC);direct_opening|
electrical|control|表示灯|ひょうじとう|Pilot lamp (panel indicator)|パイロットランプ;PL;表示ランプ;LED表示灯|個|1個|dia;colour;voltage|
electrical|control|積層信号灯|せきそうしんごうとう|Signal tower (stack light)|シグナルタワー;パトライト;積層表示灯;回転灯|台|1台|tiers;colours;voltage;buzzer|
electrical|control|回転灯|かいてんとう|Rotating warning light|パトランプ;警告灯;回転警告灯|台|1台|colour;voltage;mount|
electrical|control|ブザー|ぶざー|Buzzer|電子ブザー;警報ブザー;アラーム|個|1個|voltage;dB;mount|
electrical|control|カムスイッチ|かむすいっち|Cam switch|カム式スイッチ;切替器|個|1個|positions;current|u
electrical|control|フットスイッチ|ふっとすいっち|Foot switch|足踏みスイッチ;フットペダル|個|1個|contacts;guard|
electrical|control|ペンダントスイッチ|ぺんだんとすいっち|Pendant push-button station|ペンダント;押しボタンペンダント|個|1個|buttons|u
electrical|control|スイッチボックス(制御用)|すいっちぼっくす|Control station enclosure|操作ボックス;押釦箱;コントロールボックス|個|1個|holes;dia|
electrical|sensor|リミットスイッチ|りみっとすいっち|Limit switch|LS;リミット;位置検出スイッチ|個|1個|actuator(roller lever/plunger);contacts|
electrical|sensor|マイクロスイッチ|まいくろすいっち|Micro switch (basic switch)|マイクロ;基本スイッチ;スナップスイッチ|個|1個|actuator;rated_A|
electrical|sensor|近接センサ|きんせつせんさ|Inductive proximity sensor|近接スイッチ;近センサ;プロキシ;PRX|個|1個|dia(M8/M12/M18/M30);range_mm;output(NPN/PNP);shielded|
electrical|sensor|光電センサ|こうでんせんさ|Photoelectric sensor|光電スイッチ;光電管;PHセンサ;透過型センサ;反射型センサ|個|1個|type(through/retro/diffuse);range;output|
electrical|sensor|ファイバセンサ|ふぁいばせんさ|Fibre-optic sensor (amplifier + fibre)|ファイバーセンサ;ファイバユニット;ファイバアンプ|個|1個|amplifier/fibre;head_type|
electrical|sensor|エリアセンサ|えりあせんさ|Safety light curtain|ライトカーテン;セーフティライトカーテン|組|1組|beam_pitch;protective_height|u
electrical|sensor|圧力スイッチ|あつりょくすいっち|Pressure switch|圧力センサ;デジタル圧力スイッチ|個|1個|range;port;output|
electrical|sensor|フロートスイッチ|ふろーとすいっち|Float level switch|レベルスイッチ;液面スイッチ;フロートレス|個|1個|length;material|
electrical|sensor|熱電対|ねつでんつい|Thermocouple|熱電対;Kタイプ;K熱電対;シース熱電対;温度センサ|本|1本|type(K/J/T);sheath_dia;length|
electrical|sensor|測温抵抗体|そくおんていこうたい|Resistance temperature detector (Pt100)|Pt100;RTD;白金測温抵抗体|本|1本|class;wires(3);sheath_dia;length|
electrical|sensor|温度調節器|おんどちょうせつき|Temperature controller|温調;温調器;温度コントローラ;デジタル温度調節計|台|1台|size(48x48/96x96);input;output(relay/SSR/4-20mA)|
electrical|sensor|ロータリーエンコーダ|ろーたりーえんこーだ|Rotary encoder|エンコーダ;インクリメンタルエンコーダ|個|1個|ppr;shaft;output|
electrical|power|スイッチング電源|すいっちんぐでんげん|Switching power supply (DC24V)|直流安定化電源;パワーサプライ;電源ユニット;DC電源|台|1台|output_V(24/12/5);output_W;mount(DIN)|
electrical|power|無停電電源装置|むていでんでんげんそうち|UPS|UPS;バックアップ電源|台|1台|capacity_VA;topology|
electrical|drive|インバータ|いんばーた|Inverter (VFD)|インバーター;VFD;周波数変換器;インバ|台|1台|kW;input_V(200/400);phase|
electrical|drive|三相誘導モータ|さんそうゆうどうもーた|Three-phase induction motor|モーター;モータ;電動機;三相モータ;IM|台|1台|kW;poles;voltage;mount;efficiency(IE3)|
electrical|drive|ギヤードモータ|ぎやーどもーた|Geared motor|ギヤモータ;減速機付モータ;ギアードモーター|台|1台|kW;ratio;mount|
electrical|drive|ACスピードコントロールモータ|えーしーすぴーどこんとろーるもーた|AC speed-control motor|スピコン;スピードコントロールモーター|台|1台|W;voltage|u
electrical|drive|ステッピングモータ|すてっぴんぐもーた|Stepping motor|ステッピングモーター;パルスモータ;ステッパ|台|1台|frame;step_angle;torque|
electrical|drive|サーボモータ|さーぼもーた|AC servo motor|サーボ;サーボモーター;ACサーボ|台|1台|W;encoder;brake|
electrical|drive|サーボアンプ|さーぼあんぷ|Servo amplifier|サーボドライバ;サーボドライブ|台|1台|W;interface|
electrical|automation|PLC|ぴーえるしー|Programmable logic controller|シーケンサ;シーケンサー;プログラマブルコントローラ;PC(制御)|台|1台|io_points;cpu;comm|
electrical|automation|PLC入出力ユニット|ぴーえるしーにゅうしゅつりょくゆにっと|PLC I/O module|I/Oユニット;入力ユニット;出力ユニット;増設ユニット|台|1台|points;type|
electrical|automation|タッチパネル|たっちぱねる|HMI touch panel|表示器;HMI;GOT;プログラマブル表示器;操作パネル|台|1台|size_inch;comm|
electrical|automation|産業用イーサネットスイッチ|さんぎょうようえっとすいっち|Industrial Ethernet switch|スイッチングハブ;HUB;ハブ|台|1台|ports;mount|u
# ---------------- electrical: meters ----------------
electrical|meter|電流計|でんりゅうけい|Panel ammeter|アンメータ;電流計(盤用);A計|個|1個|range;size;ac/dc|
electrical|meter|電圧計|でんあつけい|Panel voltmeter|ボルトメータ;V計;電圧計(盤用)|個|1個|range;size|
electrical|meter|電力量計|でんりょくりょうけい|Energy meter (kWh)|kWhメーター;子メーター;積算電力計|台|1台|phase;rated_A;certified|u
electrical|meter|デジタルパネルメータ|でじたるぱねるめーた|Digital panel meter|パネルメータ;DPM|個|1個|input;digits|
# ---------------- electrical: wiring devices and lighting ----------------
electrical|device|埋込コンセント|うめこみこんせんと|Flush receptacle|コンセント;アース付コンセント;ダブルコンセント;接地コンセント|個|1個;10個入|poles;earth;rated_A(15/20)|
electrical|device|露出コンセント|ろしゅつこんせんと|Surface-mount receptacle|露出形コンセント;防雨コンセント|個|1個|earth;rated_A|
electrical|device|防水コンセント|ぼうすいこんせんと|Waterproof receptacle|防雨形コンセント;屋外コンセント|個|1個|rated_A;ip_rating|
electrical|device|引掛形コンセント|ひっかけがたこんせんと|Locking receptacle (twist-lock)|引掛コンセント;抜け止めコンセント;引掛け|個|1個|poles;rated_A;voltage(250V)|
electrical|device|埋込スイッチ|うめこみすいっち|Flush wall switch|スイッチ;片切スイッチ;3路スイッチ;ほたるスイッチ|個|1個;10個入|type(1way/3way);illuminated|
electrical|device|スイッチプレート|すいっちぷれーと|Switch / outlet cover plate|プレート;コンセントプレート;カバープレート|枚|1枚;10枚入|gang;colour|
electrical|device|引掛シーリング|ひっかけしーりんぐ|Ceiling hook receptacle|引掛シーリングボディ;角形引掛シーリング;丸形引掛シーリング|個|1個|shape|
electrical|device|接地プラグ|せっちぷらぐ|Earthed plug|アース付プラグ;3Pプラグ;ゴムキャップ;プラグ|個|1個|poles;rated_A;voltage|
electrical|device|三相プラグ|さんそうぷらぐ|Three-phase plug|引掛プラグ;動力プラグ;3P4W プラグ|個|1個|poles;rated_A|u
electrical|device|テーブルタップ|てーぶるたっぷ|Power strip|延長コード;OAタップ;電源タップ;マルチタップ|本;個|1個|outlets;length;switch;earth|
electrical|device|コードリール|こーどりーる|Cord reel|電工ドラム;ドラム;延長コードリール;コードリール(防雨)|台|1台|length(30m/50m);earth;breaker|
electrical|device|変換アダプタ|へんかんあだぷた|Plug adapter (3P to 2P)|アース変換アダプタ;3P→2P|個|1個|type|
electrical|lighting|直管LEDランプ|ちょっかんえるいーでぃーらんぷ|Straight LED tube|LED蛍光灯;直管LED;LDL40;LED管|本;箱|1本;10本入;25本入|length(40形/20形);base(G13/GX16t-5);colour_temp|
electrical|lighting|LEDベースライト|えるいーでぃーべーすらいと|LED base light (batten)|ベースライト;LED照明器具;トラフ;逆富士|台|1台|size(40形);lumen;colour_temp|
electrical|lighting|LED投光器|えるいーでぃーとうこうき|LED floodlight|投光器;作業灯(投光器);LEDライト|台|1台|W;ip_rating;cord|
electrical|lighting|LED電球|えるいーでぃーでんきゅう|LED bulb|電球;LED球;E26;E17|個|1個|base(E26/E17);W_equiv;colour|
electrical|lighting|高天井用LED照明|こうてんじょうようえるいーでぃーしょうめい|High-bay LED light|高天井照明;ハイベイ;水銀灯代替LED|台|1台|W;lumen;mount|
electrical|lighting|蛍光ランプ|けいこうらんぷ|Fluorescent tube|蛍光灯;FL40;FLR40;FHF32;直管蛍光灯|本;箱|1本;25本入|type(FL/FLR/FHF);length;colour|
electrical|lighting|誘導灯|ゆうどうとう|Exit sign light|非常口誘導灯;避難口誘導灯|台|1台|class;sides|u
electrical|lighting|非常用照明|ひじょうようしょうめい|Emergency light|非常灯;非常用照明器具|台|1台|battery;mount|u
electrical|battery|アルカリ乾電池 単3|あるかりかんでんち たんさん|Alkaline battery AA|単3;単三;LR6;AA電池;乾電池|本;パック|4本パック;10本パック;40本入|size;chemistry|
electrical|battery|アルカリ乾電池 単4|あるかりかんでんち たんよん|Alkaline battery AAA|単4;単四;LR03;AAA電池|本;パック|4本パック;10本パック|size|
electrical|battery|アルカリ乾電池 単1|あるかりかんでんち たんいち|Alkaline battery D|単1;単一;LR20;D電池|本;パック|2本パック;10本入|size|
electrical|battery|アルカリ乾電池 単2|あるかりかんでんち たんに|Alkaline battery C|単2;単二;LR14;C電池|本;パック|2本パック;10本入|size|
electrical|battery|9V形乾電池|きゅうぼるとがたかんでんち|9V battery|006P;6LR61;9V電池;角形電池|個|1個|chemistry|
electrical|battery|コイン形リチウム電池|こいんがたりちうむでんち|Lithium coin cell|ボタン電池;CR2032;CR2025;コイン電池|個;パック|1個;5個パック|size|
electrical|battery|ニッケル水素充電池|にっけるすいそじゅうでんち|NiMH rechargeable battery|充電池;エネループ(商標);充電式電池|本;パック|4本パック|size;capacity_mAh|
# ---------------- electrical: earthing ----------------
electrical|earthing|接地棒|せっちぼう|Earth rod|アース棒;接地極;アース極|本|1本|length;dia|
electrical|earthing|接地端子箱|せっちたんしばこ|Earth terminal box|接地端子盤;アースターミナル|個|1個|terminals|u
# ---------------- tools: hand tools ----------------
tools|hand|プラスドライバー|ぷらすどらいばー|Phillips screwdriver|ドライバー;十字ドライバー;+ドライバー;プラスドライバ|本|1本|size(#0/#1/#2/#3);shank_length;magnetic|
tools|hand|マイナスドライバー|まいなすどらいばー|Flat-blade screwdriver|一文字ドライバー;-ドライバー;マイナスドライバ|本|1本|tip_width;shank_length|
tools|hand|貫通ドライバー|かんつうどらいばー|Through-tang (hammer-able) screwdriver|貫通;たたけるドライバー|本|1本|size;length|
tools|hand|精密ドライバー|せいみつどらいばー|Precision screwdriver set|精密ドライバーセット;時計ドライバー|本;セット|1本;6本組|sizes|
tools|hand|ドライバービット|どらいばーびっと|Screwdriver bit|ビット;プラスビット;インパクトビット;ドライバビット|本;パック|1本;2本組;10本組|size;length;hex_shank(6.35)|
tools|hand|ソケット|そけっと|Socket (for ratchet)|ソケットレンチ;ボックス;コマ|個|1個|drive(6.35/9.5/12.7);size_mm;points(6/12);deep|
tools|hand|ラチェットハンドル|らちぇっとはんどる|Ratchet handle|ラチェット;ラチェットレンチ|本|1本|drive;length|
tools|hand|ソケットレンチセット|そけっとれんちせっと|Socket wrench set|ソケットセット;ボックスレンチセット|セット|1セット|drive;pieces|
tools|hand|スパナ|すぱな|Open-end spanner|両口スパナ;スパナー|本|1本;8本組|size_mm|
tools|hand|めがねレンチ|めがねれんち|Ring spanner (box-end wrench)|めがね;メガネレンチ;両口めがねレンチ;オフセットめがね|本|1本;セット|size_mm;offset_deg|
tools|hand|コンビネーションレンチ|こんびねーしょんれんち|Combination spanner|コンビレンチ;片目片口スパナ|本|1本;セット|size_mm|
tools|hand|ラチェットめがねレンチ|らちぇっとめがねれんち|Ratcheting ring spanner|ギアレンチ;ラチェットレンチ(めがね)|本|1本|size_mm|
tools|hand|モンキーレンチ|もんきーれんち|Adjustable wrench|モンキー;アジャスタブルレンチ;モンキースパナ|本|1本|length_mm;max_opening|
tools|hand|パイプレンチ|ぱいぷれんち|Pipe wrench|パイレン;パイプレンチ(アルミ)|本|1本|length;max_pipe|
tools|hand|六角棒レンチ|ろっかくぼうれんち|Hex key (Allen key)|六角レンチ;アーレンキー;ヘキサゴンレンチ;L形六角棒レンチ;ボールポイント|本;セット|1本;9本組|size_mm(1.5-10);ball_end;long|
tools|hand|トルクスレンチ|とるくすれんち|Torx key|ヘックスローブ;星形レンチ;T型レンチ|本;セット|1本;セット|size(T10-T50)|
tools|hand|トルクレンチ|とるくれんち|Torque wrench|プレセット形トルクレンチ;トルクレンチ(シグナル式)|本|1本|range_Nm;drive;type|
tools|hand|トルクドライバー|とるくどらいばー|Torque screwdriver|トルクドライバ;プリセットトルクドライバー|本|1本|range_cNm|
tools|hand|T形レンチ|てぃーがたれんち|T-handle wrench|Tレンチ;T型ハンドル;十字レンチ|本|1本|size|
tools|hand|ペンチ|ぺんち|Lineman's pliers|ペンチ;電工ペンチ;強力ペンチ|本|1本|length_mm|
tools|hand|ラジオペンチ|らじおぺんち|Long-nose pliers|ラジペン;ロングノーズプライヤー;先細ペンチ|本|1本|length_mm;bent|
tools|hand|ニッパー|にっぱー|Diagonal cutters (nippers)|ニッパ;斜めニッパー;精密ニッパー;プラスチックニッパー|本|1本|length;cut_capacity|
tools|hand|プライヤー|ぷらいやー|Slip-joint pliers|コンビネーションプライヤー;プライヤ|本|1本|length|
tools|hand|ウォーターポンププライヤー|うぉーたーぽんぷぷらいやー|Water-pump pliers|ウォーターポンププライヤ;WPP;グリップジョイント|本|1本|length;max_opening|
tools|hand|ロッキングプライヤー|ろっきんぐぷらいやー|Locking pliers|バイスプライヤー;グリッププライヤー;ロッキングプライヤ|本|1本|length;jaw_type|
tools|hand|スナップリングプライヤー|すなっぷりんぐぷらいやー|Snap-ring (circlip) pliers|スナップリングプライヤ;止め輪プライヤー;C形止め輪用プライヤー;穴用;軸用|本|1本|bore/shaft;size_range|
tools|hand|ワイヤーストリッパー|わいやーすとりっぱー|Wire stripper|ストリッパー;ワイヤストリッパー;皮むき|本|1本|wire_range;auto|
tools|hand|圧着工具|あっちゃくこうぐ|Crimping tool|圧着ペンチ;圧着工具;ラチェット式圧着工具;リングスリーブ用圧着工具|本|1本|terminal_type;size_range|
tools|hand|ケーブルカッター|けーぶるかったー|Cable cutter|ケーブルカッタ;ボルトクリッパ(電線用);ラチェットケーブルカッター|本|1本|max_cable_sq|
tools|hand|ボルトクリッパー|ぼるとくりっぱー|Bolt cutter|ボルトクリッパ;番線カッター|本|1本|length;max_dia|
tools|hand|ハンマー|はんまー|Engineer's hammer|片手ハンマー;両口ハンマー;玄能;金づち;トンカチ|本|1本|weight_lb;head|
tools|hand|プラスチックハンマー|ぷらすちっくはんまー|Soft-face (plastic) hammer|プラハン;ショックレスハンマー;ウレタンハンマー|本|1本|head_dia;weight|
tools|hand|ゴムハンマー|ごむはんまー|Rubber mallet|ゴムハン;ゴム槌|本|1本|weight|
tools|hand|タガネ|たがね|Cold chisel|平タガネ;たがね;チゼル|本|1本|width;length|
tools|hand|センターポンチ|せんたーぽんち|Centre punch|ポンチ;オートポンチ;センタポンチ|本|1本|tip;length|
tools|hand|ピンポンチ|ぴんぽんち|Pin punch|平行ピンポンチ;ピン抜き|本|1本;セット|dia|
tools|hand|ケガキ針|けがきばり|Scriber|けがき針;スクライバー|本|1本|tip(carbide)|
tools|hand|やすり|やすり|File (engineer's)|鉄工やすり;ヤスリ;平やすり;半丸やすり;丸やすり|本|1本;5本組|shape;length;cut(荒目/中目/細目)|
tools|hand|組やすり|くみやすり|Needle file set|精密やすり;組ヤスリ;ダイヤモンドやすり|セット|5本組;10本組|pieces;diamond|
tools|hand|弓のこ|ゆみのこ|Hacksaw frame|金のこ;金切りのこ;ハクソー;弓ノコ|本|1本|blade_length|
tools|hand|金切りばさみ|かなきりばさみ|Tin snips|金切鋏;板金ばさみ;エビ菅|本|1本|length;cut(straight/curved)|
tools|hand|カッターナイフ|かったーないふ|Utility knife|カッター;大型カッター;オルファ(商標)|本|1本|blade(L/A);lock|
tools|hand|スクレーパー|すくれーぱー|Scraper|スクレパー;ケレン;皮すき|本|1本|width|
tools|hand|ピンセット|ぴんせっと|Tweezers|ツィーザー;ピンセット(精密)|本|1本|tip;material(SUS/anti-static)|
tools|hand|マグネットピックアップ|まぐねっとぴっくあっぷ|Magnetic pick-up tool|ピックアップツール;マグネットキャッチ|本|1本|length;pull_kg|
tools|hand|点検ミラー|てんけんみらー|Inspection mirror|点検鏡;伸縮ミラー|本|1本|mirror_dia|
tools|hand|タップハンドル|たっぷはんどる|Tap wrench|タップホルダー;タップレンチ|本|1本|tap_range|
tools|hand|ダイスハンドル|だいすはんどる|Die stock|ダイスホルダー|本|1本|die_od|
tools|hand|ハンドリベッター|はんどりべったー|Hand rivet tool (blind rivets)|リベッター;ブラインドリベッター;ポップリベッター|本|1本|rivet_dia|
tools|hand|タッカー|たっかー|Staple gun (tacker)|ハンドタッカー;ガンタッカー|台|1台|staple_size|
tools|hand|グリスガン|ぐりすがん|Grease gun|グリースガン;グリスポンプ|本|1本|capacity_cc;type|
tools|hand|オイルジョッキ|おいるじょっき|Oil jug|オイラー;油差し|個|1個|capacity_L|
tools|hand|万力|まんりき|Bench vice|バイス;リードバイス;ベンチバイス|台|1台|jaw_width;swivel|
tools|hand|マシンバイス|ましんばいす|Machine vice|精密バイス;ミーリングバイス|台|1台|jaw_width;opening|
tools|hand|C形クランプ|しーがたくらんぷ|C-clamp|シャコ万;シャコマン;C型クランプ|個|1個|opening;depth|
tools|hand|F形クランプ|えふがたくらんぷ|F-clamp (bar clamp)|Fクランプ;L型クランプ;バークランプ|個|1個|opening;depth|
tools|hand|クイックバークランプ|くいっくばーくらんぷ|Quick-release bar clamp|クイッククランプ;ワンハンドクランプ|個|1個|opening|
tools|hand|工具箱|こうぐばこ|Tool box|ツールボックス;工具ケース;両開きツールボックス|個|1個|size;material(steel/plastic)|
tools|hand|工具セット|こうぐせっと|Tool set|ツールセット;メンテナンスキット|セット|1セット|pieces|
tools|hand|工具差し|こうぐさし|Tool pouch / holster|腰袋;工具袋;ツールバッグ;電工バッグ|個|1個|pockets;material|
# ---------------- tools: measuring ----------------
tools|measuring|ノギス|のぎす|Vernier caliper|ノギス(標準);M形ノギス;バーニヤキャリパ|本|1本|range_mm(150/200/300);resolution|
tools|measuring|デジタルノギス|でじたるのぎす|Digital caliper|デジノギ;デジタルキャリパー;クーラントプルーフノギス|本|1本|range_mm;resolution(0.01);output|
tools|measuring|外側マイクロメータ|そとがわまいくろめーた|Outside micrometer|マイクロメーター;マイクロ;外マイク|本|1本|range(0-25/25-50);resolution(0.001)|
tools|measuring|内側マイクロメータ|うちがわまいくろめーた|Inside micrometer|内マイク;三点式内側マイクロメータ;ボアマイク|本|1本|range|
tools|measuring|ダイヤルゲージ|だいやるげーじ|Dial indicator|ダイヤルインジケータ;ダイゲ;DG|個|1個|range_mm;resolution|
tools|measuring|てこ式ダイヤルゲージ|てこしきだいやるげーじ|Dial test indicator (lever)|ピックテスタ;テストインジケータ;てこ式|個|1個|range;resolution|
tools|measuring|シリンダゲージ|しりんだげーじ|Bore gauge (dial cylinder gauge)|シリンダーゲージ;内径ゲージ;ボアゲージ|セット|1セット|range|
tools|measuring|ハイトゲージ|はいとげーじ|Height gauge|デジタルハイトゲージ;ハイゲ|台|1台|range;resolution|
tools|measuring|マグネットベース|まぐねっとべーす|Magnetic base (indicator stand)|マグスタンド;マグネットスタンド|個|1個|holding_N;arm|
tools|measuring|すきまゲージ|すきまげーじ|Feeler gauge|シクネスゲージ;フィラーゲージ;隙間ゲージ|組|1組|leaves;range|
tools|measuring|ピッチゲージ|ぴっちげーじ|Thread pitch gauge|ねじピッチゲージ;スレッドゲージ|組|1組|metric/inch|
tools|measuring|ピンゲージ|ぴんげーじ|Pin gauge|ピンゲ;栓ゲージ(ピン);プラグゲージ(ピン)|本;セット|1本;セット|dia_mm;class|
tools|measuring|ねじゲージ|ねじげーじ|Thread plug / ring gauge|ねじプラグゲージ;ねじリングゲージ;GPIP;限界ゲージ|個|1個|thread;class(6H/6g);GO/NOGO|
tools|measuring|ブロックゲージ|ぶろっくげーじ|Gauge block set|ゲージブロック;ヨハンソン|セット|1セット|pieces;grade|u
tools|measuring|直尺|ちょくしゃく|Steel rule|スケール;ステンレス直尺;定規;金尺|本|1本|length(150/300/600/1000)|
tools|measuring|コンベックス|こんべっくす|Tape measure (steel)|メジャー;巻尺;スケール(巻);コンベ|個|1個|length(3.5m/5.5m);width|
tools|measuring|巻尺|まきじゃく|Long tape measure (fibre/steel)|ロングテープ;ガラス繊維巻尺;鋼製巻尺|個|1個|length(30m/50m)|
tools|measuring|水平器|すいへいき|Spirit level|水準器;レベル;水平計;精密水準器|個|1個|length;accuracy|
tools|measuring|スコヤ|すこや|Engineer's square|直角定規;台付スコヤ;スコヤー;曲尺|個|1個|size;grade|
tools|measuring|プロトラクター|ぷろとらくたー|Protractor (bevel)|分度器;角度計;デジタル角度計;ベベルプロトラクター|個|1個|range;resolution|
tools|measuring|テスター|てすたー|Multimeter|マルチメータ;デジタルテスター;回路計;DMM|台|1台|cat_rating;true_rms|
tools|measuring|クランプメーター|くらんぷめーたー|Clamp meter|クランプテスター;クランプ電流計;リーククランプ|台|1台|range_A;ac/dc;leakage|
tools|measuring|検電器|けんでんき|Voltage detector (non-contact)|検電ペン;低圧検電器;ネオン検電器|本|1本|voltage_range|
tools|measuring|絶縁抵抗計|ぜつえんていこうけい|Insulation resistance tester|メガー;メガ;絶縁計;メガテスタ|台|1台|test_V(125/250/500/1000)|
tools|measuring|接地抵抗計|せっちていこうけい|Earth resistance tester|アーステスタ;接地抵抗テスタ|台|1台|method|
tools|measuring|検相器|けんそうき|Phase rotation tester|相回転計;検相計|台|1台|contact/non-contact|
tools|measuring|放射温度計|ほうしゃおんどけい|Infrared thermometer|非接触温度計;赤外線温度計;IR温度計|台|1台|range;emissivity|
tools|measuring|デジタル温度計|でじたるおんどけい|Contact thermometer (with probe)|温度計;熱電対温度計;表面温度計|台|1台|probe;range|
tools|measuring|照度計|しょうどけい|Lux meter|ルクスメーター;照度メーター|台|1台|range_lx|
tools|measuring|騒音計|そうおんけい|Sound level meter|サウンドレベルメータ;デシベル計|台|1台|class|
tools|measuring|回転計|かいてんけい|Tachometer|タコメータ;ハンドタコ;非接触回転計|台|1台|contact/optical|
tools|measuring|振動計|しんどうけい|Vibration meter|バイブロメータ;振動チェッカー|台|1台|parameters|u
tools|measuring|硬さ試験機|かたさしけんき|Hardness tester (portable)|硬度計;ショア硬度計;ゴム硬度計;デュロメータ|台|1台|scale|u
tools|measuring|膜厚計|まくあつけい|Coating thickness gauge|膜厚測定器;塗膜厚計|台|1台|substrate|
tools|measuring|圧力計|あつりょくけい|Pressure gauge|圧力ゲージ;ブルドン管圧力計;プレッシャーゲージ|個|1個|range_MPa;dial_dia;thread|
tools|measuring|温湿度計|おんしつどけい|Thermo-hygrometer|湿度計;デジタル温湿度計|個|1個|range;logging|
tools|measuring|はかり|はかり|Weighing scale|台はかり;デジタルはかり;電子天秤;計量器|台|1台|capacity;resolution;certified|u
tools|measuring|ルーペ|るーぺ|Magnifier loupe|拡大鏡;虫眼鏡;スケールルーペ|個|1個|magnification|
# ---------------- tools: power tools ----------------
tools|power|インパクトドライバー|いんぱくとどらいばー|Impact driver (cordless)|インパクト;インパクトドライバ;充電インパクト|台|1台|voltage(14.4/18V);torque;kit/body|
tools|power|ドリルドライバー|どりるどらいばー|Drill driver (cordless)|ドライバドリル;充電ドリル;電動ドライバー|台|1台|voltage;chuck_mm;clutch|
tools|power|振動ドリル|しんどうどりる|Hammer (percussion) drill|震動ドリル;振動ドライバードリル|台|1台|chuck;power|
tools|power|ハンマードリル|はんまーどりる|Rotary hammer|ハンマドリル;SDSドリル;コンクリートドリル|台|1台|shank(SDS-plus/max);power|
tools|power|電動ドリル|でんどうどりる|Electric drill (corded)|電気ドリル;ドリル(電動);鉄工ドリル(電動)|台|1台|chuck;power_W|
tools|power|ディスクグラインダー|でぃすくぐらいんだー|Angle grinder|グラインダー;ディスクグラインダ;サンダー(俗);100mmグラインダー|台|1台|disc_dia(100/125/180);power;switch|
tools|power|ベビーサンダー|べびーさんだー|Small angle grinder (100mm)|ベビサン;小型グラインダー|台|1台|disc_dia|u
tools|power|ストレートグラインダー|すとれーとぐらいんだー|Die grinder|ルーター(電動);リューター;ハンドグラインダー;ダイグラインダー|台|1台|collet;rpm|
tools|power|オービタルサンダー|おーびたるさんだー|Orbital sander|サンダー;ランダムオービットサンダー;ポリッシャー|台|1台|pad_size;orbit|
tools|power|ポリッシャー|ぽりっしゃー|Polisher|電動ポリッシャー;バフ掛け機|台|1台|pad_dia|
tools|power|丸のこ|まるのこ|Circular saw|丸ノコ;電気丸のこ;チップソー機|台|1台|blade_dia;cut_depth|
tools|power|チップソー|ちっぷそー|Carbide-tipped saw blade|丸のこ刃;鉄工用チップソー;チップソー刃|枚|1枚|dia;teeth;material|
tools|power|ジグソー|じぐそー|Jigsaw|ジグソ;電動糸のこ|台|1台|stroke;orbital|
tools|power|ジグソーブレード|じぐそーぶれーど|Jigsaw blade|ジグソー刃;ブレード|パック|5本入|material;tpi|
tools|power|レシプロソー|れしぷろそー|Reciprocating saw|セーバーソー;セーバソー;電気のこぎり|台|1台|stroke;power|
tools|power|セーバーソーブレード|せーばーそーぶれーど|Reciprocating saw blade|レシプロソー刃;セーバソーブレード|パック|5本入|length;tpi;material|
tools|power|ポータブルバンドソー|ぽーたぶるばんどそー|Portable band saw|ハンディバンドソー;バンドソー|台|1台|capacity_mm|
tools|power|バンドソー刃|ばんどそーば|Band-saw blade|帯のこ;バンドソーブレード;帯鋸刃|本|1本;5本入|length;width;tpi|
tools|power|高速切断機|こうそくせつだんき|Abrasive cut-off saw|切断機;メタルカッター;カッター(切断機)|台|1台|disc_dia(305/355);power|
tools|power|卓上ボール盤|たくじょうぼーるばん|Bench drill press|ボール盤;卓上ボール盤;ボール盤(卓上)|台|1台|capacity_mm;table|
tools|power|磁気ボール盤|じきぼーるばん|Magnetic drill|アトラ(俗);マグネットボール盤;磁気ボール盤|台|1台|capacity;cutter_type|
tools|power|ヒートガン|ひーとがん|Heat gun|ホットガン;熱風機;ヒーティングガン;工業用ドライヤー|台|1台|temp_range;power|
tools|power|グルーガン|ぐるーがん|Hot-melt glue gun|ホットボンド;グルー;ホットメルト|台|1台|stick_dia|
tools|power|はんだごて|はんだごて|Soldering iron|半田ごて;ハンダゴテ;温調はんだごて;はんだステーション|本;台|1本|power_W;temp_control|
tools|power|はんだごて用こて先|はんだごてようこてさき|Soldering iron tip|こて先;チップ(はんだごて)|本|1本|shape;series|
tools|power|はんだ吸取器|はんだすいとりき|Desoldering pump|ソルダーポンプ;はんだ吸取機|本;台|1本|type|
tools|power|電動ブロワ|でんどうぶろわ|Electric blower|ブロワー;ブロア;送風機(ハンディ)|台|1台|airflow;cordless|
tools|power|業務用掃除機|ぎょうむようそうじき|Industrial vacuum cleaner|集じん機;集塵機;バキュームクリーナー;乾湿両用掃除機|台|1台|capacity_L;wet/dry;power|
tools|power|集じん機フィルタ|しゅうじんきふぃるた|Vacuum / dust extractor filter|集塵機フィルター;紙パック;フィルターバッグ|個;袋|1個;5枚入|model_fit|u
tools|power|高圧洗浄機|こうあつせんじょうき|Pressure washer|高圧洗浄器;ケルヒャー(俗);ジェットクリーナー|台|1台|pressure_MPa;flow;power|
tools|power|充電式バッテリー|じゅうでんしきばってりー|Power-tool battery pack|バッテリー;リチウムイオンバッテリ;電池パック|個|1個|voltage;capacity_Ah;platform|
tools|power|充電器|じゅうでんき|Power-tool battery charger|急速充電器;バッテリーチャージャ|台|1台|voltage;platform|
tools|power|作業灯|さぎょうとう|Work light|LEDワークライト;作業ライト;ハンドランプ|台|1台|lumen;cordless|
tools|power|ヘッドライト|へっどらいと|Head torch|ヘッドランプ;頭灯|個|1個|lumen;battery|
tools|power|懐中電灯|かいちゅうでんとう|Flashlight|ハンディライト;LEDライト;トーチ|個|1個|lumen;battery|
# ---------------- tools: air tools ----------------
tools|air|エアブローガン|えあぶろーがん|Air blow gun|エアガン;ダスターガン;ブローガン|本|1本|nozzle;coupler|
tools|air|エアインパクトレンチ|えあいんぱくとれんち|Air impact wrench|エアーインパクト;エアインパクト|台|1台|drive;torque|
tools|air|エアラチェットレンチ|えあらちぇっとれんち|Air ratchet|エアーラチェット|台|1台|drive|
tools|air|エアホース|えあほーす|Air hose|エアーホース;ウレタンホース(エア);コイルホース|本;巻|10m;20m;巻|inner_dia;length;coupler|
tools|air|エアカプラ|えあかぷら|Air quick coupler|カプラ;カプラー;ワンタッチカプラ;ハイカプラ(商標)|個|1個|type(socket/plug);thread;hose_dia|
tools|air|エアホースリール|えあほーすりーる|Air hose reel|ホースリール(エア);エアーリール|台|1台|length;auto_rewind|
tools|air|コンプレッサー|こんぷれっさー|Air compressor|エアコンプレッサー;コンプレッサ;空気圧縮機|台|1台|tank_L;pressure;power;oil-free|u
# ---------------- tools: cutting tools ----------------
tools|cutting|鉄工用ドリル|てっこうようどりる|HSS twist drill|ドリル;ストレートドリル;ドリル刃;ハイスドリル;鉄工ドリル|本;パック|1本;10本入|dia_mm;length;coating|
tools|cutting|コバルトドリル|こばるとどりる|Cobalt HSS drill (for stainless)|ステンレス用ドリル;Coドリル;コバルトハイス|本|1本|dia_mm|
tools|cutting|超硬ドリル|ちょうこうどりる|Solid carbide drill|超硬ソリッドドリル;カーバイドドリル|本|1本|dia_mm;length_xD;coolant|
tools|cutting|六角軸ドリル|ろっかくじくどりる|Hex-shank drill (for impact driver)|六角軸鉄工ドリル;インパクト用ドリル|本|1本|dia_mm|
tools|cutting|コンクリートドリル|こんくりーとどりる|Masonry drill bit|振動ドリル用ビット;SDSビット;コンクリートビット|本|1本|dia;shank(SDS-plus);length|
tools|cutting|ステップドリル|すてっぷどりる|Step drill|タケノコドリル;スパイラルステップドリル;ユニビット(俗)|本|1本|range_mm|
tools|cutting|ホールソー|ほーるそー|Hole saw|ホルソー;ホールソ;バイメタルホールソー;超硬ホールソー|本;個|1個|dia_mm;depth|
tools|cutting|面取りカッター|めんとりかったー|Countersink / chamfer cutter|皿取錐;カウンターシンク;面取り工具|本|1本|dia;angle(90)|
tools|cutting|センタードリル|せんたーどりる|Centre drill|センタドリル;センター穴ドリル|本|1本|dia;angle|
tools|cutting|リーマ|りーま|Reamer|リーマー;ハンドリーマ;マシンリーマ;チャッキングリーマ|本|1本|dia_mm;tolerance(H7)|
tools|cutting|タップ|たっぷ|Tap|ハンドタップ;スパイラルタップ;ポイントタップ;ねじ切りタップ;盛上げタップ|本|1本;3本組|thread(M3-M20);pitch;type|
tools|cutting|ダイス|だいす|Threading die|丸ダイス;ねじ切りダイス|個|1個|thread;od|
tools|cutting|エンドミル(超硬)|えんどみる|Solid carbide end mill|エンドミル;超硬エンドミル;スクエアエンドミル;フラットエンドミル;EM|本|1本|dia_mm;flutes(2/3/4);length;coating|
tools|cutting|エンドミル(ハイス)|えんどみる|HSS end mill|ハイスエンドミル;ハイスEM;コバルトエンドミル|本|1本|dia_mm;flutes|
tools|cutting|ボールエンドミル|ぼーるえんどみる|Ball-nose end mill|ボールEM;ボール;R付きエンドミル|本|1本|radius;flutes|
tools|cutting|ラジアスエンドミル|らじあすえんどみる|Corner-radius end mill|ラジアスEM;コーナーRエンドミル|本|1本|dia;corner_R|
tools|cutting|面取りエンドミル|めんとりえんどみる|Chamfer mill|面取りカッター(エンドミル);C面取り|本|1本|angle;dia|
tools|cutting|正面フライス|しょうめんふらいす|Face mill (indexable)|フライスカッター;フェイスミル|個|1個|dia;inserts|
tools|cutting|スローアウェイチップ|すろーあうぇいちっぷ|Indexable insert|チップ;インサート;刃先交換チップ;スローアウェイ|個;箱|10個入|iso_code;grade;chipbreaker|
tools|cutting|旋削ホルダ|せんさくほるだ|Turning tool holder|バイトホルダ;外径ホルダ;ホルダー|本|1本|iso_code;shank|
tools|cutting|ろう付けバイト|ろうづけばいと|Brazed carbide turning tool|バイト;付刃バイト;超硬バイト|本|1本|shape;shank|
tools|cutting|ハイスバイト|はいすばいと|HSS tool bit|完成バイト;ハイス角バイト|本|1本|size|
tools|cutting|突っ切りバイト|つっきりばいと|Parting tool|突切りバイト;溝入れバイト|本|1本|width|u
tools|cutting|ドリルチャック|どりるちゃっく|Drill chuck|キーレスチャック;チャック|個|1個|capacity;mount|
tools|cutting|コレットチャック|これっとちゃっく|Collet chuck holder|ミーリングチャック;コレットホルダ|個|1個|taper(BT40);collet_type|
tools|cutting|コレット|これっと|Collet|ストレートコレット;ERコレット|個|1個|type(ER16/ER32);dia|
tools|cutting|ドリルソケット|どりるそけっと|Morse taper sleeve / socket|スリーブ;ドリルスリーブ;モールステーパー|個|1個|mt_size|
tools|cutting|プルスタッド|ぷるすたっど|Pull stud (retention knob)|プルボルト;リテンションノブ|個|1個|taper;angle|u
tools|cutting|ねじ切りバイト|ねじきりばいと|Threading insert / tool|ねじ切りチップ;スレッディング|個;本|1個|pitch;form|u
# ---------------- tools: material handling and access ----------------
tools|handling|脚立|きゃたつ|Stepladder|はしご兼用脚立;踏み台;作業台(脚立)|台|1台|height;steps;material|
tools|handling|はしご|はしご|Ladder (extension)|梯子;2連はしご;伸縮はしご|台|1台|length;material|
tools|handling|踏み台|ふみだい|Step stool / work platform|作業台;ステップ;足場台|台|1台|height|
tools|handling|台車|だいしゃ|Hand truck (platform trolley)|運搬車;平台車;キャリー;手押し台車|台|1台|load_kg;size;casters|
tools|handling|ハンドリフト|はんどりふと|Pallet truck|ハンドパレット;ハンパレ;ビシャモン(俗);パレットトラック|台|1台|load_kg;fork_length|
tools|handling|チェーンブロック|ちぇーんぶろっく|Chain hoist|チェンブロック;手動ホイスト;チェーンホイスト|台|1台|capacity_t;lift_m|
tools|handling|レバーブロック|ればーぶろっく|Lever hoist|レバーホイスト;レバブロ|台|1台|capacity_t|
tools|handling|油圧ジャッキ|ゆあつじゃっき|Hydraulic jack|ジャッキ;ボトルジャッキ;爪ジャッキ|台|1台|capacity_t|
tools|handling|運搬用コンテナ|うんぱんようこんてな|Plastic tote / container|コンテナ;サンボックス;オリコン;通い箱;番重|個|1個|size;volume_L;foldable|
tools|handling|パーツケース|ぱーつけーす|Parts organiser|部品箱;パーツボックス;小物入れ;ビンラック|個|1個|compartments|
tools|handling|工具ワゴン|こうぐわごん|Tool cart|ツールワゴン;ワゴン|台|1台|drawers;load|
tools|handling|作業台|さぎょうだい|Workbench|ワークテーブル;作業テーブル;軽量作業台|台|1台|size;load|
tools|handling|キャスター|きゃすたー|Caster wheel|自在キャスター;固定キャスター;車輪|個|1個;4個組|wheel_dia;load;brake|
tools|handling|定盤|じょうばん|Surface plate|石定盤;鋳鉄定盤;精密定盤|台|1台|size;grade|u
tools|handling|Vブロック|ぶいぶろっく|V-block|Vブロック(マグネット);精密Vブロック|個;組|1組|size|
tools|handling|刻印|こくいん|Steel letter/number punch set|数字刻印;英字刻印;打刻ポンチ|セット|1セット|char_size|
# ---------------- consumables: gloves and PPE ----------------
consumables|ppe|軍手|ぐんて|Cotton work gloves|綿手袋;作業用手袋;スムス手袋;7ゲージ軍手|双;ダース;袋|12双入;1ダース|gauge(7G/10G);size|
consumables|ppe|すべり止め手袋|すべりどめてぶくろ|Dotted grip gloves|イボ付手袋;滑り止め軍手;ドット手袋|双;袋|12双入|size|
consumables|ppe|ニトリル背抜き手袋|にとりるせぬきてぶくろ|Nitrile-palm-coated gloves|背抜き手袋;ニトリル手袋(作業用);コーティング手袋;ゴム引き手袋|双;袋|10双入;12双入|size;coating;liner|
consumables|ppe|ウレタン手袋|うれたんてぶくろ|PU-coated gloves (precision)|PU手袋;ウレタンコート手袋;トップフィット手袋|双;袋|10双入|size|
consumables|ppe|使い捨てニトリル手袋|つかいすてにとりるてぶくろ|Disposable nitrile gloves|ニトリルグローブ;ディスポ手袋;使い捨て手袋;パウダーフリー手袋|箱;ケース|100枚入;1000枚入|size;thickness;powder_free;colour|
consumables|ppe|使い捨てポリエチレン手袋|つかいすてぽりえちれんてぶくろ|Disposable PE gloves|ポリ手袋;PE手袋;ポリエチ手袋|箱;袋|100枚入|size|
consumables|ppe|耐切創手袋|たいせっそうてぶくろ|Cut-resistant gloves|カットレジスタント手袋;防刃手袋|双|1双;10双入|level(EN388/ISO13997);size|
consumables|ppe|革手袋|かわてぶくろ|Leather work gloves|皮手袋;牛革手袋;床革手袋|双|1双;10双入|size;leather|
consumables|ppe|溶接用革手袋|ようせつようかわてぶくろ|Welding gloves|溶接手袋;溶接用手袋;5本指革手袋|双|1双;10双入|length;size|
consumables|ppe|耐熱手袋|たいねつてぶくろ|Heat-resistant gloves|耐熱グローブ;アラミド手袋|双|1双|max_temp;size|
consumables|ppe|耐油手袋|たいゆてぶくろ|Oil/chemical-resistant gloves|耐油ビニール手袋;耐溶剤手袋;化学防護手袋;ゴム手袋|双|1双;10双入|length;material;size|
consumables|ppe|電気用ゴム手袋|でんきようごむてぶくろ|Electrical insulating gloves|絶縁手袋;低圧ゴム手袋;電気用手袋|双|1双|voltage_class;size|
consumables|ppe|保護めがね|ほごめがね|Safety glasses|保護メガネ;セーフティグラス;防じんメガネ;ゴーグル|個|1個;10個入|type(spectacle/goggle);anti-fog;over-glasses|
consumables|ppe|保護ゴーグル|ほごごーぐる|Safety goggles|ゴーグル;防災ゴーグル;化学用ゴーグル|個|1個|ventilation;anti-fog|
consumables|ppe|防災面|ぼうさいめん|Face shield|フェイスシールド;防護面|個|1個|material;mount|
consumables|ppe|溶接面|ようせつめん|Welding helmet|溶接用保護面;自動遮光面;溶接マスク;ハンドシールド|個|1個|shade;auto-darkening|
consumables|ppe|溶接遮光レンズ|ようせつしゃこうれんず|Welding filter lens / cover plate|遮光ガラス;遮光プレート;カバーガラス;素ガラス|枚|1枚;10枚入|shade(#10-#13);size|
consumables|ppe|使い捨て防じんマスク|つかいすてぼうじんますく|Disposable particulate respirator (DS2)|DS2マスク;DS1マスク;防じんマスク(使い捨て);N95相当|枚;箱|10枚入;20枚入|class(DS1/DS2/DS3);valve|
consumables|ppe|取替え式防じんマスク|とりかえしきぼうじんますく|Reusable particulate respirator|取替式防じんマスク;RS2;RL2;半面形|個|1個|class(RS2/RL2/RL3)|
consumables|ppe|防じんマスク用フィルタ|ぼうじんますくようふぃるた|Replacement respirator filter|取替えフィルタ;防じんフィルター|枚;袋|5枚入;10枚入|model_fit;class|
consumables|ppe|防毒マスク用吸収缶|ぼうどくますくようきゅうしゅうかん|Gas-mask canister / cartridge|吸収缶;有機ガス用吸収缶;防毒マスク吸収缶|個|1個;2個入|gas_type(organic/acid);model_fit|
consumables|ppe|サージカルマスク|さーじかるますく|Disposable face mask|不織布マスク;使い捨てマスク;衛生マスク|箱|50枚入|size;ply|
consumables|ppe|耳栓|みみせん|Ear plugs|イヤープラグ;フォーム耳栓;紐付き耳栓|組;箱|1組;100組入|nrr_dB;corded|
consumables|ppe|イヤーマフ|いやーまふ|Ear muffs|防音イヤーマフ;防音保護具|個|1個|nrr_dB;helmet_mount|
consumables|ppe|ヘルメット|へるめっと|Safety helmet|保護帽;安全帽;作業用ヘルメット;メット|個|1個|certified(飛来落下/墜落時/電気用);colour;vent|
consumables|ppe|ヘルメット用あごひも|へるめっとようあごひも|Helmet chin strap / liner|あごひも;内装体;汗取りパッド|個|1個;10個入|model_fit|u
consumables|ppe|安全靴|あんぜんぐつ|Safety shoes|セーフティシューズ;プロテクティブスニーカー;安全スニーカー;鉄芯入り靴|足|1足|size_cm;JIS_T8101;toe_cap;esd|
consumables|ppe|長靴|ながぐつ|Rubber boots (safety)|安全長靴;ゴム長;耐油長靴|足|1足|size;steel_toe|
consumables|ppe|フルハーネス型墜落制止用器具|ふるはーねすがたついらくせいしようきぐ|Full-body harness (fall arrest)|フルハーネス;墜落制止用器具;安全帯;ハーネス|個;組|1組|size;lanyard;standard|
consumables|ppe|胴ベルト型墜落制止用器具|どうべるとがたついらくせいしようきぐ|Waist belt fall-arrest device|胴ベルト型安全帯;安全帯(胴ベルト)|個|1個|height_limit|u
consumables|ppe|安全ベスト|あんぜんべすと|High-visibility vest|反射ベスト;安全チョッキ;蛍光ベスト|着|1着;10着入|colour;size|
consumables|ppe|使い捨て保護服|つかいすてほごふく|Disposable coverall|タイベック(商標);防護服;ディスポつなぎ;使い捨てつなぎ|着;箱|1着;25着入|size;type|
consumables|ppe|腕カバー|うでかばー|Sleeve covers|アームカバー;袖カバー;耐切創アームカバー|双|1双;10双入|material;length|
consumables|ppe|前掛け|まえかけ|Work apron|エプロン;ゴム前掛け;溶接用前掛け|枚|1枚|material;size|
consumables|ppe|帽子(防寒・防暑)|ぼうし|Work cap|作業帽;キャップ;ヘルメットインナー|個|1個|size|u
# ---------------- consumables: wiping and cleaning ----------------
consumables|cleaning|ウエス|うえす|Rags (cloth wiping rags)|ウェス;布ウエス;メリヤスウエス;リサイクルウエス;雑巾|kg;袋;箱|1kg;5kg;10kg|material(cotton/knit);colour(white/mixed)|
consumables|cleaning|不織布ワイパー|ふしょくふわいぱー|Non-woven industrial wipers|不織布ウエス;工業用ワイパー;キムタオル(商標);ワイプオール(商標)|箱;ケース|50枚入;12箱入|size;ply;lint-free|
consumables|cleaning|紙ワイパー|かみわいぱー|Paper wipers (precision)|キムワイプ(商標);精密ワイパー;ペーパーワイパー|箱;ケース|200枚入;60箱入|size|
consumables|cleaning|ペーパータオル|ぺーぱーたおる|Paper towels|ハンドタオル;ペーパーハンドタオル;紙タオル|袋;ケース|200枚入;7200枚入|size;ply|
consumables|cleaning|トイレットペーパー|といれっとぺーぱー|Toilet paper|トイレットロール;ロールペーパー|ロール;ケース|12ロール;60ロール|length_m;ply|
consumables|cleaning|ゴミ袋 45L|ごみぶくろ|Rubbish bags 45 L|ポリ袋;ごみ袋;ゴミ袋45リットル;半透明ゴミ袋|袋;箱;ケース|10枚入;50枚入;1000枚入|volume_L;thickness_mm;colour|
consumables|cleaning|ゴミ袋 90L|ごみぶくろ|Rubbish bags 90 L|90Lポリ袋;大型ゴミ袋|袋;ケース|10枚入;300枚入|volume_L;thickness|
consumables|cleaning|オイル吸着材|おいるきゅうちゃくざい|Oil-absorbent mat / pads|油吸着マット;吸着マット;オイルマット;油取りシート|枚;箱|100枚入|size;oil-only/universal|
consumables|cleaning|オイル吸着フェンス|おいるきゅうちゃくふぇんす|Absorbent boom / sock|オイルフェンス;吸着ソックス;吸着ロール|本;箱|1本;4本入|length;dia|u
consumables|cleaning|中性洗剤|ちゅうせいせんざい|Neutral detergent|洗剤;工業用洗剤;マルチクリーナー|本;缶|1L;4L;18L|volume|
consumables|cleaning|アルカリ洗浄剤|あるかりせんじょうざい|Alkaline degreasing cleaner|脱脂剤(水系);アルカリ脱脂剤;パーツ洗浄剤(水系)|缶|4L;18L|volume;concentration|
consumables|cleaning|工業用ハンドソープ|こうぎょうようはんどそーぷ|Industrial hand cleaner|ハンドクリーナー;油汚れ用石けん;手洗い洗剤;スクラブ入り|本;缶|500mL;2.5kg;4L|volume;scrub|
consumables|cleaning|消毒用アルコール|しょうどくようあるこーる|Hand sanitiser (ethanol)|アルコール消毒液;手指消毒剤;エタノール|本|500mL;1L;5L|concentration|
consumables|cleaning|イソプロピルアルコール|いそぷろぴるあるこーる|Isopropyl alcohol (IPA)|IPA;イソプロパノール;2-プロパノール|本;缶|500mL;1L;18L|purity|
consumables|cleaning|パーツクリーナー|ぱーつくりーなー|Parts cleaner spray (degreaser)|ブレーキクリーナー;脱脂スプレー;パークリ;洗浄スプレー|本;ケース|840mL;20本入|volume;quick-dry;non-chlorinated|
consumables|cleaning|接点復活剤|せってんふっかつざい|Contact cleaner|接点洗浄剤;コンタクトスプレー;接点クリーナー|本|1本|volume|
consumables|cleaning|ブラシ(掃除用)|ぶらし|Cleaning brush|デッキブラシ;手ブラシ;ほうき;チリトリ|本|1本|type|u
consumables|cleaning|モップ|もっぷ|Mop|モップ替糸;モップヘッド|本|1本|width|u
# ---------------- consumables: packing ----------------
consumables|packing|OPPテープ|おーぴーぴーてーぷ|OPP packing tape (clear)|梱包テープ;透明テープ;OPP粘着テープ;セロテープ(商標;別物)|巻;ケース|1巻;50巻入|width(48mm);length(100m);thickness|
consumables|packing|布粘着テープ|ぬのねんちゃくてーぷ|Cloth duct tape|布ガムテープ;布テープ;ガムテ|巻;ケース|1巻;30巻入|width(50mm);length(25m);colour|
consumables|packing|クラフトテープ|くらふとてーぷ|Kraft paper tape|紙ガムテープ;紙テープ;クラフト粘着テープ|巻;ケース|1巻;50巻入|width;length|
consumables|packing|養生テープ|ようじょうてーぷ|Low-tack protection tape|養生用テープ;PEクロステープ;仮止めテープ|巻;ケース|1巻;30巻入|width;colour|
consumables|packing|マスキングテープ|ますきんぐてーぷ|Masking tape|マスキング;マステ;紙マスキングテープ;塗装用マスキング|巻;パック|1巻;10巻パック|width;temp_resistant|
consumables|packing|両面テープ|りょうめんてーぷ|Double-sided tape|両面粘着テープ;強力両面テープ;VHB(商標);発泡両面テープ|巻|1巻|width;thickness;foam|
consumables|packing|アルミテープ|あるみてーぷ|Aluminium foil tape|アルミ粘着テープ;アルミ箔テープ|巻|1巻|width;length|
consumables|packing|床用ラインテープ|ゆかようらいんてーぷ|Floor marking tape|ラインテープ;床表示テープ;区画線テープ;トラテープ|巻|1巻|width;colour;durable|
consumables|packing|ストレッチフィルム|すとれっちふぃるむ|Stretch film|ストレッチ;ラップ(梱包);パレットラップ;荷崩れ防止フィルム|巻;ケース|1巻;6巻入|width_mm;thickness_um;length_m|
consumables|packing|PPバンド|ぴーぴーばんど|PP strapping band|PPバンド;梱包バンド;荷造りバンド;結束バンド(梱包)|巻|1巻|width;length;machine/hand|
consumables|packing|ダンボール箱|だんぼーるばこ|Corrugated cardboard box|段ボール;ダンボール;カートン;梱包箱|枚;束|10枚;20枚|size;flute(A/B);strength|
consumables|packing|気泡緩衝材|きほうかんしょうざい|Bubble wrap|プチプチ(商標);エアキャップ(商標);エアパッキン;ミナパック|巻|1巻|width;length;bubble_size|
consumables|packing|緩衝材|かんしょうざい|Packing filler (paper/foam)|ミラーマット;発泡シート;更紙;ペーパークッション|巻;袋|1巻|type|
consumables|packing|ポリ袋|ぽりぶくろ|Polyethylene bag|ポリエチレン袋;規格袋;平袋;チャック付ポリ袋;ユニパック(商標)|袋;箱|100枚入|size;thickness;zip|
consumables|packing|防錆袋|ぼうせいぶくろ|VCI anti-rust bag|防錆フィルム;気化性防錆袋;防錆紙|袋;巻|100枚入;巻|size|
consumables|packing|乾燥剤|かんそうざい|Desiccant|シリカゲル;除湿剤;乾燥剤(梱包用)|袋;缶|100個入|size_g|
consumables|packing|ラベルシール|らべるしーる|Labels (sheet / roll)|ラベル;シール;タックシール;テプラテープ(商標);ラベルテープ|枚;巻;箱|100シート;1巻|size;printer_type|
consumables|packing|荷札|にふだ|Shipping tag|タグ;荷札(針金付)|枚;束|100枚入|size|
consumables|packing|パレット|ぱれっと|Pallet|プラパレット;木製パレット;樹脂パレット|枚|1枚|size(1100x1100);material;load|u
# ---------------- consumables: marking and writing ----------------
consumables|marking|油性マーカー|ゆせいまーかー|Permanent marker|マジック(商標);油性ペン;マジックインキ(商標);太字マーカー|本;箱|1本;10本入|tip;colour|
consumables|marking|ペイントマーカー|ぺいんとまーかー|Paint marker|ペイントマーカ;不透明マーカー;工業用マーカー|本;箱|1本;10本入|tip;colour|
consumables|marking|石筆|せきひつ|Soapstone marker (welding chalk)|チョーク(鉄工);ソープストーン;石筆ホルダー|本;箱|1本;100本入|shape|
consumables|marking|マーキングスプレー|まーきんぐすぷれー|Marking spray paint|ラッカースプレー(マーキング);逆さスプレー|本|1本|colour|
consumables|marking|コピー用紙|こぴーようし|Copy paper A4|A4用紙;PPC用紙;OA用紙;コピー紙|冊;箱|500枚;2500枚|size(A4/A3);weight_gsm|
consumables|marking|けがき用塗料|けがきようとりょう|Layout fluid (marking blue)|けがきインキ;ケガキ青ニス;ダイカム|本|1本|colour|u
# ---------------- consumables: lubricants and chemicals ----------------
consumables|lubricant|浸透潤滑剤|しんとうじゅんかつざい|Penetrating lubricant spray|潤滑スプレー;CRC 5-56(商標);ゴーゴー(俗);防錆潤滑剤;浸透潤滑スプレー|本;ケース|420mL;20本入|volume;food_grade|
consumables|lubricant|防錆剤|ぼうせいざい|Rust preventive|防錆スプレー;防錆油;さび止め油|本;缶|420mL;18L|film(oil/wax);duration|
consumables|lubricant|シリコンスプレー|しりこんすぷれー|Silicone spray|シリコーンスプレー;シリコン潤滑剤|本|1本|volume|
consumables|lubricant|離型剤|りけいざい|Mould release agent|離型スプレー;シリコン離型剤;非シリコン離型剤|本;缶|1本;18L|silicone/non-silicone|
consumables|lubricant|リチウムグリース|りちうむぐりーす|Lithium grease|グリース;グリス;万能グリース;マルチパーパスグリース;EPグリース|本;缶;個|80g;400g;16kg|NLGI(No.1/No.2);base;EP|
consumables|lubricant|ウレアグリース|うれあぐりーす|Urea grease (bearings, high temp)|ウレアグリス;高温グリース|本;缶|400g;16kg|NLGI|u
consumables|lubricant|モリブデングリース|もりぶでんぐりーす|Molybdenum grease|二硫化モリブデングリース;モリブデングリス|本;缶|400g;16kg|NLGI|
consumables|lubricant|食品機械用グリース|しょくひんきかいようぐりーす|Food-grade grease (NSF H1)|H1グリース;食品機械用潤滑剤|本;缶|400g|NSF_H1|
consumables|lubricant|作動油|さどうゆ|Hydraulic oil|油圧作動油;ハイドロリックオイル;油圧オイル|缶;ドラム|20L;200L|ISO_VG(32/46/68);anti-wear|
consumables|lubricant|ギヤ油|ぎやゆ|Industrial gear oil|ギヤーオイル;ギアオイル;工業用ギヤ油|缶;ドラム|20L;200L|ISO_VG(150/220/320)|
consumables|lubricant|摺動面油|しゅうどうめんゆ|Slideway oil|しゅう動面油;スライドウェイオイル|缶;ドラム|20L;200L|ISO_VG(68/220)|
consumables|lubricant|スピンドル油|すぴんどるゆ|Spindle oil|スピンドルオイル;マシン油|缶|4L;20L|ISO_VG(2/10/22)|
consumables|lubricant|マシン油|ましんゆ|Machine oil|マシンオイル;機械油|本;缶|1L;4L;20L|ISO_VG|
consumables|lubricant|コンプレッサ油|こんぷれっさゆ|Compressor oil|コンプレッサーオイル|缶|4L;20L|ISO_VG;type|
consumables|lubricant|チェーンオイル|ちぇーんおいる|Chain lubricant|チェーンスプレー;チェーンルブ|本|1本|volume|
consumables|lubricant|真空ポンプ油|しんくうぽんぷゆ|Vacuum pump oil|ロータリーポンプ油;真空ポンプオイル|缶|1L;4L|grade|u
consumables|cutting_fluid|水溶性切削油|すいようせいせっさくゆ|Water-soluble cutting fluid|クーラント;水溶性クーラント;切削液;エマルション|缶;ドラム|18L;200L|type(emulsion/soluble/solution);dilution|
consumables|cutting_fluid|不水溶性切削油|ふすいようせいせっさくゆ|Neat (straight) cutting oil|切削油;油性切削油;ストレートオイル|缶;ドラム|18L;200L|viscosity;additives|
consumables|cutting_fluid|タッピングペースト|たっぴんぐぺーすと|Tapping paste / fluid|タッピングオイル;タップ油;切削ペースト|本;缶|1本;1kg|form|
consumables|cutting_fluid|研削液|けんさくえき|Grinding fluid|研削油;研削用クーラント|缶|18L|type|u
consumables|chemical|シンナー|しんなー|Thinner|ラッカーシンナー;うすめ液;塗料用シンナー|缶|4L;16L|type|
consumables|chemical|アセトン|あせとん|Acetone|アセトン(工業用)|本;缶|500mL;18L|purity|
consumables|chemical|白灯油|しろとうゆ|Kerosene (cleaning)|灯油;洗い油|缶|18L|grade|u
consumables|chemical|錆取り剤|さびとりざい|Rust remover|サビ取り剤;除錆剤;錆落とし|本;缶|1L;4L|type|
consumables|chemical|冷却スプレー|れいきゃくすぷれー|Freeze spray|コールドスプレー;冷却剤|本|1本|volume|u
consumables|chemical|探傷剤|たんしょうざい|Dye penetrant kit|浸透探傷剤;カラーチェック;PT|セット;本|1セット|type|
consumables|chemical|漏れ検知剤|もれけんちざい|Leak detection spray|リークチェッカー;ガス漏れ検知剤;リークテスター|本|1本|volume|
# ---------------- consumables: adhesives and sealants ----------------
consumables|adhesive|瞬間接着剤|しゅんかんせっちゃくざい|Cyanoacrylate adhesive|アロンアルフア(商標);シアノアクリレート;瞬着|本;個|1本;20g;50g|viscosity|
consumables|adhesive|エポキシ接着剤|えぽきしせっちゃくざい|Epoxy adhesive (2-part)|2液性接着剤;エポキシ;ボンドクイック(商標)|セット;個|1セット|cure_time|
consumables|adhesive|嫌気性ねじゆるみ止め剤|けんきせいねじゆるみどめざい|Thread-locking adhesive|ねじロック;ロックタイト(商標);スリーボンド(商標);ゆるみ止め|本|10mL;50mL|strength(low/medium/high)|
consumables|adhesive|液状ガスケット|えきじょうがすけっと|Liquid gasket / flange sealant|FIPG;液ガス;シール剤|本|100g;200g|type(silicone/anaerobic)|
consumables|adhesive|シリコーンシーラント|しりこーんしーらんと|Silicone sealant|シリコンシーラント;シリコンコーキング;コーキング;バスコーク|本;箱|330mL;10本入|colour;cure|
consumables|adhesive|変成シリコンシーラント|へんせいしりこんしーらんと|Modified-silicone sealant (paintable)|変成シリコン;MSシーラント|本|330mL|colour|
consumables|adhesive|シールテープ|しーるてーぷ|PTFE thread-seal tape|配管用シールテープ;テフロンテープ(商標);PTFEテープ|巻;パック|1巻;10巻入|width;length|
consumables|adhesive|配管用シール剤|はいかんようしーるざい|Pipe thread sealant (paste)|ヘルメシール(俗);ねじシール剤;配管シール|本;缶|1本|type|u
consumables|adhesive|ホットメルトスティック|ほっとめるとすてぃっく|Hot-melt glue sticks|グルースティック;ホットボンド用スティック|袋|30本入|dia;length|
consumables|adhesive|接着剤(合成ゴム系)|せっちゃくざい|Contact adhesive (rubber-based)|ボンドG17(商標);ゴム系接着剤;万能接着剤|本;缶|170mL;1kg|type|
# ---------------- consumables: paint ----------------
consumables|paint|スプレー塗料|すぷれーとりょう|Spray paint (lacquer)|ラッカースプレー;スプレーペイント;缶スプレー|本|300mL;420mL|colour;finish|
consumables|paint|さび止め塗料|さびどめとりょう|Anti-rust primer|錆止めペイント;防錆塗料;プライマー|缶|0.7L;4kg;16kg|colour|
consumables|paint|ジンクリッチペイント|じんくりっちぺいんと|Zinc-rich paint (cold galvanising)|ローバル(商標);亜鉛末塗料;常温亜鉛めっき|本;缶|420mL;1kg|form|
consumables|paint|床用塗料|ゆかようとりょう|Floor paint|床塗料;エポキシ床塗料;防塵塗装|缶|4kg;16kg|type|u
consumables|paint|刷毛|はけ|Paint brush|ハケ;塗装用刷毛;ペイントブラシ|本|1本;10本入|width|
consumables|paint|塗装用ローラー|とそうようろーらー|Paint roller|ローラー;ローラーハンドル;スモールローラー|本|1本;10本入|width;nap|
# ---------------- consumables: abrasives ----------------
consumables|abrasive|研磨紙|けんまし|Sandpaper|サンドペーパー;紙やすり;ペーパー;空研ぎペーパー|枚;袋|10枚入;100枚入|grit(#60-#1000);size|
consumables|abrasive|耐水ペーパー|たいすいぺーぱー|Waterproof abrasive paper|耐水サンドペーパー;水研ぎペーパー;耐水研磨紙|枚;袋|10枚入;100枚入|grit;size|
consumables|abrasive|研磨布|けんまふ|Abrasive cloth|布ペーパー;研磨布紙;エメリークロス;布やすり|枚;巻|10枚入;1巻|grit;width|
consumables|abrasive|ディスクペーパー|でぃすくぺーぱー|Fibre disc (sanding disc)|ファイバーディスク;サンディングディスク;マジック式ディスク|枚;箱|10枚入;25枚入|dia;grit;backing|
consumables|abrasive|多羽根ディスク|たばねでぃすく|Flap disc|フラップディスク;マジックディスク(俗);ペーパーディスク|枚;箱|10枚入|dia;grit;angle|
consumables|abrasive|切断砥石|せつだんといし|Cut-off wheel|切断ディスク;カッティングホイール;薄刃砥石;切断砥石(グラインダー用)|枚;箱|10枚入;25枚入|dia(105/125/305/355);thickness;material(steel/stainless)|
consumables|abrasive|オフセット砥石|おふせっとといし|Grinding wheel (depressed centre)|研削砥石;オフセット;グラインダー砥石;研削ディスク|枚;箱|10枚入;25枚入|dia;thickness;grit|
consumables|abrasive|平形砥石|ひらがたといし|Bench grinder wheel|両頭グラインダー用砥石;平型砥石;GC砥石;WA砥石|枚|1枚|dia;width;bore;grain(A/WA/GC);grit|
consumables|abrasive|軸付砥石|じくつきといし|Mounted point|軸付き砥石;マウンテッドポイント;砥石(軸付)|本;箱|1本;10本入|shape;dia;grit|
consumables|abrasive|ダイヤモンドカッター|だいやもんどかったー|Diamond cutting blade|ダイヤカッター;ダイヤモンドホイール|枚|1枚|dia;type(segment/rim)|
consumables|abrasive|ワイヤーブラシ|わいやーぶらし|Wire brush (hand)|ワイヤブラシ;鉄ブラシ;真鍮ブラシ;ステンレスブラシ|本|1本|rows;wire(steel/SUS/brass)|
consumables|abrasive|カップブラシ|かっぷぶらし|Cup wire brush (grinder)|カップワイヤーブラシ;ねじり線カップブラシ|個|1個|dia;thread(M10);wire|
consumables|abrasive|ホイールブラシ|ほいーるぶらし|Wheel wire brush|ワイヤーホイール;軸付ワイヤーブラシ|個|1個|dia;shank|
consumables|abrasive|不織布研磨材|ふしょくふけんまざい|Non-woven abrasive pad|ナイロン研磨材;スコッチブライト(商標);研磨パッド;ナイロンディスク|枚;箱|10枚入|grade;size|
consumables|abrasive|バフ|ばふ|Buffing wheel|バフ研磨;布バフ;サイザルバフ|個|1個|dia;material|
consumables|abrasive|研磨剤|けんまざい|Polishing compound|コンパウンド;青棒;白棒;ピカール(商標)|本;個|1本|grade|
consumables|abrasive|ダイヤモンドやすり|だいやもんどやすり|Diamond file|ダイヤモンドヤスリ;電着ダイヤやすり|本|1本;セット|shape;grit|
# ---------------- consumables: welding and soldering ----------------
consumables|welding|被覆アーク溶接棒|ひふくあーくようせつぼう|Covered arc welding electrode|溶接棒;アーク溶接棒;被覆棒;イルミナイト系;低水素系|箱;kg|5kg入;20kg入|dia_mm(2.6/3.2/4.0);JIS_class;position|
consumables|welding|ソリッドワイヤ|そりっどわいや|Solid wire (MAG/CO2)|MAGワイヤ;CO2ワイヤ;溶接ワイヤー;YGW|巻|20kg巻;10kg巻|dia_mm(0.9/1.0/1.2);JIS_class(YGW11/YGW12)|
consumables|welding|フラックス入りワイヤ|ふらっくすいりわいや|Flux-cored wire|FCW;フラックスワイヤ;ノンガスワイヤ(セルフシールド)|巻|12.5kg巻;20kg巻;0.8kg巻|dia_mm;JIS_class;gas/no-gas|
consumables|welding|ステンレス溶接ワイヤ|すてんれすようせつわいや|Stainless welding wire|ステンレスワイヤ;SUS溶接ワイヤ|巻|12.5kg巻|dia;grade(308/309/316)|
consumables|welding|TIG溶加棒|てぃぐようかぼう|TIG filler rod|溶加棒;TIG棒;ティグ棒|kg;箱|1kg;5kg|dia;material(steel/SUS/Al)|
consumables|welding|タングステン電極棒|たんぐすてんでんきょくぼう|Tungsten electrode|タングステン棒;TIG電極;セリタン;トリタン;ランタン入り|本;箱|10本入|dia(1.6/2.4/3.2);type(Ce/La/Th);length|
consumables|welding|溶接チップ|ようせつちっぷ|Contact tip (MIG/MAG torch)|コンタクトチップ;チップ(溶接);トーチチップ|個;箱|10個入;100個入|wire_dia;thread;length|
consumables|welding|溶接ノズル|ようせつのずる|Gas nozzle (MIG/MAG torch)|ノズル;シールドノズル;トーチノズル|個|1個;10個入|torch_type;inner_dia|
consumables|welding|オリフィス|おりふぃす|Gas orifice / diffuser|インシュレータ;ガスディフューザ;セラミックオリフィス|個|1個;10個入|torch_type|u
consumables|welding|TIGセラミックノズル|てぃぐせらみっくのずる|TIG ceramic nozzle (cup)|アルミナノズル;TIGノズル;セラミックノズル|個|1個;10個入|size;torch_type|
consumables|welding|スパッタ付着防止剤|すぱったふちゃくぼうしざい|Anti-spatter spray / compound|スパッタ防止剤;ノズルジェル;スパッタ防止スプレー|本;缶|1本;18L|form|
consumables|welding|溶接用シールドガス(炭酸ガス)|ようせつようしーるどがす|CO2 shielding gas|炭酸ガス;CO2ガス;液化炭酸ガス|本|30kg容器;ボンベ|cylinder_size;rental|u
consumables|welding|溶接用シールドガス(アルゴン)|ようせつようしーるどがす|Argon shielding gas|アルゴンガス;Arガス;アルゴン;混合ガス(Ar+CO2)|本|7m3容器;ボンベ|cylinder_size;mix|u
consumables|welding|ガス切断用火口|がすせつだんようひぐち|Cutting torch tip|火口;切断火口;ガス切断チップ|個|1個|size;gas(acetylene/propane)|u
consumables|welding|プラズマ切断用電極・ノズル|ぷらずませつだんようでんきょく|Plasma cutting electrode / nozzle|プラズマ電極;プラズマノズル;プラズマチップ|個;箱|1個;5個入|torch_type;amperage|
consumables|welding|やに入りはんだ|やにいりはんだ|Flux-cored solder wire|はんだ;糸はんだ;鉛フリーはんだ;ハンダ|巻;個|100g;500g;1kg|dia;alloy(Sn-Ag-Cu/Sn-Pb);flux|
consumables|welding|フラックス|ふらっくす|Soldering flux|はんだ付けフラックス;ペーストフラックス;液体フラックス|本|1本|type|
consumables|welding|はんだ吸取線|はんだすいとりせん|Desoldering braid|ソルダーウィック;吸取線;はんだ吸い取りテープ|巻|1巻|width;length|
# ---------------- consumables: fasteners ----------------
consumables|fastener|六角ボルト|ろっかくぼると|Hex bolt|ボルト;六角ボルト(全ねじ);半ねじボルト;ヘックスボルト|本;箱|1本;100本入;1箱|thread(M6-M24);length;material(SS/SUS);strength(4.8/8.8/10.9);finish|
consumables|fastener|六角穴付きボルト|ろっかくあなつきぼると|Socket head cap screw|キャップスクリュー;キャップボルト;CAP;六角穴付ボルト;CS|本;箱;袋|1本;100本入|thread;length;material;strength(12.9)|
consumables|fastener|六角ナット|ろっかくなっと|Hex nut|ナット;六角ナット1種;3種ナット;ヘックスナット|個;箱|100個入;1箱|thread;type(1種/3種);material|
consumables|fastener|ナイロンナット|ないろんなっと|Nylon-insert lock nut|Uナット;ナイロンインサートナット;セルフロックナット|個;箱|100個入|thread;material|
consumables|fastener|フランジナット|ふらんじなっと|Flange nut|セレートナット;フランジ付ナット|個;箱|100個入|thread|
consumables|fastener|袋ナット|ふくろなっと|Acorn (cap) nut|袋ナット;キャップナット;化粧ナット|個;箱|100個入|thread;material|
consumables|fastener|蝶ナット|ちょうなっと|Wing nut|ちょうナット;ウイングナット|個;箱|100個入|thread|
consumables|fastener|平座金|ひらざがね|Flat washer|ワッシャー;平ワッシャー;丸座金;ワッシャ|個;箱|100個入;1箱|nominal(M3-M24);material;finish|
consumables|fastener|ばね座金|ばねざがね|Spring washer|スプリングワッシャー;SW;スプリングワッシャ|個;箱|100個入|nominal;material|
consumables|fastener|歯付座金|はつきざがね|Toothed lock washer|菊座金;歯付きワッシャー;外歯;内歯|個;箱|100個入|nominal;type|
consumables|fastener|なべ小ねじ|なべこねじ|Pan-head machine screw|小ねじ;ナベ小ネジ;なべ;十字穴付きなべ小ねじ;ビス|本;箱;袋|100本入;1000本入|thread(M2-M8);length;material|
consumables|fastener|皿小ねじ|さらこねじ|Countersunk machine screw|皿ビス;皿ネジ;サラ小ネジ|本;箱|100本入|thread;length|
consumables|fastener|トラス小ねじ|とらすこねじ|Truss-head machine screw|トラス;トラスビス|本;箱|100本入|thread;length|
consumables|fastener|座金組込み小ねじ|ざがねくみこみこねじ|Machine screw with captive washers (SEMS)|セムス;セムスねじ;P=2;P=3|本;箱|100本入|thread;length;washer_type|
consumables|fastener|タッピンねじ|たっぴんねじ|Tapping screw|タッピングビス;タッピングネジ;1種タッピン;Bタイト;Pタイト(商標)|本;箱|100本入;1000本入|size(2-6);length;type(1種/2種/3種/4種)|
consumables|fastener|ドリルねじ|どりるねじ|Self-drilling screw|ドリルビス;ドリルネジ;テクスビス(商標);ジャックポイント|本;箱|100本入;500本入|dia;length;head|
consumables|fastener|コーススレッド|こーすすれっど|Coarse-thread wood screw|コーススレッド;木ねじ;ビス(木工)|本;箱|100本入;1000本入|dia;length|
consumables|fastener|止めねじ|とめねじ|Set screw|イモネジ;ホーローセット;六角穴付止めねじ;セットスクリュー;芋ねじ|本;箱|100本入|thread;length;point(cup/flat/cone)|
consumables|fastener|全ねじボルト|ぜんねじぼると|Threaded rod|寸切りボルト;寸切;全ネジ;W3/8;ズンギリ|本|1m;2m;1本|thread(W3/8/M10/M12);length;finish|
consumables|fastener|アイボルト|あいぼると|Eye bolt|吊りボルト;アイナット|個|1個|thread;wll|
consumables|fastener|アンカーボルト|あんかーぼると|Post-installed anchor|あと施工アンカー;オールアンカー(商標);ホールインアンカー(商標);ケミカルアンカー(商標);芯棒打込み式アンカー|本;箱|1本;50本入|dia;length;type|
consumables|fastener|ブラインドリベット|ぶらいんどりべっと|Blind rivet|ポップリベット(商標);リベット;ブラインドリベット(アルミ)|本;箱|1000本入;100本入|dia;grip;material|
consumables|fastener|ボルト用キャップ|ぼるとようきゃっぷ|Bolt cap|ボルトキャップ;保護キャップ|個;袋|100個入|size|u
consumables|fastener|インサートナット|いんさーとなっと|Threaded insert|ヘリサート(商標);ねじインサート;E-SERT;リベットナット|個;箱|100個入|thread;length|
consumables|fastener|割りピン|わりぴん|Split pin (cotter pin)|コッターピン;割ピン|本;箱|100本入|dia;length;material|
consumables|fastener|スプリングピン|すぷりんぐぴん|Spring (roll) pin|ロールピン;スプリングピン(SUS)|本;箱|100本入|dia;length|
consumables|fastener|平行ピン|へいこうぴん|Dowel pin (parallel)|ノックピン;ダウエルピン;平行ピン(焼入)|本;袋|10本入;100本入|dia;length;tolerance(m6/h7)|
consumables|fastener|テーパーピン|てーぱーぴん|Taper pin|テーパピン;テーパーピン(めねじ付)|本|10本入|dia;length|u
consumables|fastener|Eリング|いーりんぐ|E-ring (retaining clip)|E形止め輪;Eクリップ;E型止め輪|個;袋|100個入|nominal(shaft_dia)|
consumables|fastener|C形止め輪(軸用)|しーがたとめわ|External circlip (shaft)|軸用C形止め輪;Cリング(軸);スナップリング(軸)|個;袋|50個入;100個入|shaft_dia|
consumables|fastener|C形止め輪(穴用)|しーがたとめわ|Internal circlip (bore)|穴用C形止め輪;Cリング(穴);スナップリング(穴)|個;袋|50個入|bore_dia|
consumables|fastener|キー|きー|Machine key (parallel)|平行キー;キー材;沈みキー|本;個|1本;10個入|size(bxh);length|
consumables|fastener|結束線|けっそくせん|Tie wire (annealed)|なまし鉄線;番線;バインド線;鉄線|巻;束|1巻;1束|gauge(#8-#22);finish|
consumables|fastener|ステンレス針金|すてんれすはりがね|Stainless wire|SUS針金;ステン線;ロックワイヤー|巻|1巻|dia|
# ---------------- consumables: sealing, bearings, transmission ----------------
consumables|seal|Oリング|おーりんぐ|O-ring|オーリング;パッキン(Oリング);P規格;G規格|個;袋|1個;10個入;100個入|series(P/G/S/V/AS568);size;material(NBR/FKM/EPDM);hardness|
consumables|seal|Oリングセット|おーりんぐせっと|O-ring assortment kit|Oリングキット;Oリング詰め合わせ|セット|1セット|series;material|
consumables|seal|オイルシール|おいるしーる|Oil seal (rotary shaft)|オイルシール;ダストシール;TC形;SC形|個|1個|id;od;width;type;material|
consumables|seal|ガスケット|がすけっと|Flange gasket|パッキン;ジョイントシート;ノンアスベストガスケット;うず巻形ガスケット|枚|1枚;10枚入|flange_std(JIS10K);size;material|
consumables|seal|ゴム板|ごむいた|Rubber sheet|ゴムシート;天然ゴム板;NBR板|枚;m|1枚;切り売り|thickness;width;material|
consumables|seal|グランドパッキン|ぐらんどぱっきん|Gland packing|パッキン(グランド);編組パッキン;テフロンパッキン|m;巻|1m;巻|size;material|u
consumables|bearing|深溝玉軸受|ふかみぞたまじくうけ|Deep-groove ball bearing|ベアリング;玉軸受;ボールベアリング;6000番;6200番;6300番|個|1個;10個入|designation(6205ZZ);seal(ZZ/DDU/LLU/2RS);clearance(C3)|
consumables|bearing|ベアリングユニット|べありんぐゆにっと|Bearing unit (pillow block)|ピローブロック;UCP;UCF;軸受ユニット;ユニット軸受|個|1個|type(UCP/UCF/UCFL);bore|
consumables|bearing|円すいころ軸受|えんすいころじくうけ|Tapered roller bearing|テーパーローラーベアリング;円錐ころ軸受;30200番|個|1個|designation|
consumables|bearing|ニードル軸受|にーどるじくうけ|Needle roller bearing|ニードルベアリング;針状ころ軸受|個|1個|id;od;width|u
consumables|bearing|含油軸受|がんゆじくうけ|Oil-impregnated sintered bush|オイレスブッシュ(商標);焼結含油軸受;ドライベアリング;ブッシュ|個|1個;10個入|id;od;length;flange|
consumables|bearing|リニアブッシュ|りにあぶっしゅ|Linear ball bushing|リニアベアリング;LMブッシュ;リニアボールブッシュ|個|1個|shaft_dia;type|
consumables|transmission|Vベルト|ぶいべると|V-belt|Vベルト;A形;B形;M形;ファンベルト;赤ベルト(俗)|本|1本|section(M/A/B/C);length_inch|
consumables|transmission|細幅Vベルト|ほそはばぶいべると|Narrow V-belt|3Vベルト;5Vベルト;SPZ;ウェッジベルト|本|1本|section;length|
consumables|transmission|タイミングベルト|たいみんぐべると|Timing belt|歯付ベルト;同期ベルト;HTD;S8M;MXL;XL;L|本|1本|pitch;teeth;width|
consumables|transmission|ローラーチェーン|ろーらーちぇーん|Roller chain|チェーン;RS40;RS50;RS60;ドライブチェーン|本;m|1本(3m);切り売り|size(RS35-RS120);strands;links|
consumables|transmission|チェーン継手|ちぇーんつぎて|Chain connecting link|継手リンク;JL;OL;ジョイント(チェーン);オフセットリンク|個|1個|size;type|
consumables|transmission|スプロケット|すぷろけっと|Sprocket|スプロケ;ハブ付スプロケット|個|1個|size;teeth;bore|u
consumables|transmission|カップリング|かっぷりんぐ|Shaft coupling|軸継手;フレキシブルカップリング;ジョーカップリング;チェーンカップリング|個|1個|bore;type|
consumables|transmission|ガススプリング|がすすぷりんぐ|Gas spring|ガスダンパー;ガスストラット;ダンパー|本|1本|force_N;stroke|u
consumables|transmission|圧縮コイルばね|あっしゅくこいるばね|Compression spring|コイルスプリング;ばね;スプリング|個;袋|1個;10個入|od;wire;free_length|
consumables|transmission|引張コイルばね|ひっぱりこいるばね|Extension spring|引張ばね;引きばね;テンションスプリング|個;袋|1個;10個入|od;wire;length|
# ---------------- consumables: pneumatics, piping, filtration ----------------
consumables|pneumatic|ウレタンチューブ|うれたんちゅーぶ|Polyurethane air tube|エアチューブ;ウレタンホース;PUチューブ;チューブ(エア)|巻|20m巻;100m巻|od(4/6/8/10/12);colour|
consumables|pneumatic|ナイロンチューブ|ないろんちゅーぶ|Nylon air tube|ナイロンホース;PAチューブ|巻|20m巻;100m巻|od;colour|
consumables|pneumatic|ワンタッチ管継手|わんたっちかんつぎて|Push-in fitting|ワンタッチ継手;ワンタッチフィッティング;ワンタッチ;チューブ継手|個;袋|1個;10個入|tube_od;thread(R1/8-R1/2/M5);shape(straight/elbow/tee)|
consumables|pneumatic|スピードコントローラ|すぴーどこんとろーら|Speed controller (flow control)|スピコン(空圧);流量調整弁;スピードコントローラー|個|1個|tube_od;thread|
consumables|pneumatic|エアフィルタエレメント|えあふぃるためれめんと|Air-line filter element|フィルタエレメント;エレメント(空圧)|個|1個|micron;model_fit|
consumables|pneumatic|サイレンサ|さいれんさ|Pneumatic silencer|消音器;マフラー(空圧);サイレンサー|個|1個;10個入|thread|
consumables|pneumatic|電磁弁|でんじべん|Solenoid valve (pneumatic)|ソレノイドバルブ;電磁バルブ;方向制御弁;SV|個|1個|ports;positions;voltage;size|
consumables|pneumatic|エアシリンダ|えあしりんだ|Air cylinder|シリンダ;エアーシリンダー;空圧シリンダ|本;個|1本|bore;stroke;mount;switch|
consumables|pneumatic|吸着パッド|きゅうちゃくぱっど|Vacuum suction cup|真空パッド;バキュームパッド;吸盤|個|1個;10個入|dia;material|
consumables|piping|ボールバルブ|ぼーるばるぶ|Ball valve|ボール弁;バルブ;コック;ミニボールバルブ|個|1個|size(1/4-2);thread;material(brass/SUS)|
consumables|piping|ゲートバルブ|げーとばるぶ|Gate valve|仕切弁;ゲート弁|個|1個|size;pressure(10K);material|
consumables|piping|チェックバルブ|ちぇっくばるぶ|Check valve|逆止弁;逆止め弁;リフトチェッキ;スイングチェッキ|個|1個|size;type|
consumables|piping|ねじ込み管継手|ねじこみかんつぎて|Threaded pipe fitting (malleable iron)|継手;エルボ;チーズ;ソケット(配管);ニップル;ブッシング(配管);白継手;黒継手|個|1個;10個入|size;shape;finish(白/黒)|
consumables|piping|ステンレス管継手|すてんれすかんつぎて|Stainless threaded fitting|SUS継手;ステン継手|個|1個|size;shape|
consumables|piping|ホースバンド|ほーすばんど|Hose clamp|ホースクランプ;ホースバンド(SUS);ウォームギヤ式バンド|個;袋|1個;10個入|range_mm;width;material|
consumables|piping|耐圧ホース|たいあつほーす|Reinforced PVC hose (braided)|ブレードホース;耐圧ブレードホース;糸入りホース|m;巻|切り売り;50m巻|id;od;pressure|
consumables|piping|サクションホース|さくしょんほーす|Suction hose|吸引ホース;サクション;ダクトホース|m;巻|切り売り;20m巻|id|u
consumables|piping|ホースニップル|ほーすにっぷる|Hose barb (nipple)|竹の子;タケノコ;ホースジョイント;ホースニップル(真鍮)|個|1個|thread;hose_id|
consumables|piping|配管保温材|はいかんほおんざい|Pipe insulation|保温チューブ;ラッキング;保温筒|本|1本(1m)|pipe_size;thickness|u
consumables|filtration|オイルフィルタ|おいるふぃるた|Oil filter (hydraulic/engine)|オイルエレメント;油圧フィルタ;作動油フィルタ;ラインフィルタ|個|1個|model_fit;micron|u
consumables|filtration|クーラントフィルタ|くーらんとふぃるた|Coolant filter / bag|クーラントろ過フィルタ;フィルターバッグ;バッグフィルタ;ペーパーフィルタ|個;枚|1個;10枚入|micron;size|u
consumables|filtration|エアコンフィルタ|えあこんふぃるた|Air-conditioner / ventilation filter|プレフィルタ;空調フィルタ;換気フィルタ|枚;巻|1枚;1巻|size;grade|u
# ---------------- consumables: rigging and safety signage ----------------
consumables|rigging|ナイロンスリング|ないろんすりんぐ|Webbing sling|スリングベルト;ベルトスリング;吊りベルト;ナイロンスリング(両端アイ)|本|1本|width;length;wll|
consumables|rigging|ワイヤーロープ|わいやーろーぷ|Wire rope (sling)|ワイヤスリング;玉掛ワイヤ;ワイヤー|本;m|1本;切り売り|dia;length;construction|
consumables|rigging|シャックル|しゃっくる|Shackle|シャックル(SB/SC);ネジシャックル|個|1個|size;wll;type(B/D)|
consumables|rigging|ラッシングベルト|らっしんぐべると|Ratchet tie-down strap|荷締めベルト;荷締機;ラッシング|本|1本|width;length;load|
consumables|rigging|ロープ|ろーぷ|Rope (PP / nylon)|PPロープ;トラロープ;標識ロープ;クレモナロープ(商標)|巻;本|1巻;切り売り|dia;length;material|
consumables|safety|カラーコーン|からーこーん|Traffic cone|パイロン;コーン;セーフティコーン;三角コーン|本|1本|height;colour|
consumables|safety|コーンバー|こーんばー|Cone bar|コーンバー(伸縮);バー|本|1本|length|
consumables|safety|安全標識|あんぜんひょうしき|Safety sign|標識;安全表示;ステッカー;注意喚起看板|枚|1枚|size;material;message|
consumables|safety|救急箱|きゅうきゅうばこ|First-aid kit|救急セット;ファーストエイド|個|1個|contents|u
consumables|safety|消火器|しょうかき|Fire extinguisher|粉末消火器;ABC消火器;強化液消火器|本|1本|type;size(10型)|u
consumables|safety|熱中症対策品|ねっちゅうしょうたいさくひん|Heat-stress supplies|塩タブレット;経口補水液;冷却タオル|袋;箱|1袋|type|u
"""

# Part-number patterns. scope "standard" = JIS/ISO/IEC designation, "maker" = a published
# manufacturer series format. confidence "medium" = format varies, use as a hint only.
MPN = {
    "VVF flat cable (600V PVC sheathed)": [
        ("standard", None, r"^VVF\s*(1\.6|2\.0|2\.6)(mm)?\s*[-×xX]\s*[2-4]C$", "VVF2.0-3C", "high"),
    ],
    "CV cable (XLPE insulated PVC sheathed)": [
        ("standard", None, r"^CV\s*\d+(\.\d+)?\s*(sq|SQ|mm2|㎟)?\s*[-×xX]\s*[1-4]C$", "CV5.5sq-3C", "high"),
    ],
    "IV wire (600V PVC insulated)": [
        ("standard", None, r"^IV\s*(\d+(\.\d+)?)\s*(mm|sq|SQ|mm2)?$", "IV5.5sq", "high"),
    ],
    "VCTF round PVC cabtyre cord": [
        ("standard", None, r"^VCTF\s*\d+(\.\d+)?\s*(sq|SQ|mm2)?\s*[-×xX]\s*\d{1,2}C$", "VCTF0.75sq-2C", "high"),
    ],
    "CVV control cable": [
        ("standard", None, r"^CVV(-S)?\s*\d+(\.\d+)?\s*(sq|SQ|mm2)?\s*[-×xX]\s*\d{1,2}C$", "CVV2sq-4C", "high"),
    ],
    "Ring crimp terminal (R type)": [
        ("standard", None, r"^R\s?(1\.25|2|3\.5|5\.5|8|14|22|38|60|100)-\d{1,2}(\.\d)?$", "R5.5-6", "high"),
    ],
    "Fork crimp terminal (Y type)": [
        ("standard", None, r"^(1\.25|2|3\.5|5\.5)Y-\d(\.\d)?$", "1.25Y-4", "medium"),
    ],
    "Closed-end splice": [
        ("standard", None, r"^CE-?(1|2|5|8)$", "CE2", "high"),
    ],
    "PF flexible conduit (self-extinguishing)": [
        ("standard", None, r"^PF-?[SD]?\s?(14|16|22|28|36|42)$", "PF-S16", "medium"),
    ],
    "VE rigid PVC conduit": [
        ("standard", None, r"^VE\s?(14|16|22|28|36|42|54|70|82)$", "VE22", "high"),
    ],
    "E threadless steel conduit": [
        ("standard", None, r"^E\s?(19|25|31|39|51|63|75)$", "E25", "high"),
    ],
    "Moulded-case circuit breaker (MCCB)": [
        ("maker", "Mitsubishi Electric", r"^NF\d{2,4}-[A-Z]{2,4}", "NF32-SV 3P 20A", "high"),
        ("maker", "Fuji Electric", r"^BW\d{2,4}[A-Z]{3}", "BW32AAG-3P020", "medium"),
    ],
    "Earth-leakage circuit breaker (ELCB)": [
        ("maker", "Mitsubishi Electric", r"^NV\d{2,4}-[A-Z]{2,4}", "NV32-SV 3P 20A 30MA", "high"),
        ("maker", "Fuji Electric", r"^EW\d{2,4}[A-Z]{3}", "EW32AAG-3P020", "medium"),
    ],
    "Magnetic contactor": [
        ("maker", "Mitsubishi Electric", r"^S-T\d{2,3}[A-Z]*", "S-T21 AC200V", "high"),
        ("maker", "Fuji Electric", r"^SC-E?\d{2,3}[A-Z]?", "SC-E02", "medium"),
    ],
    "Magnetic starter (contactor + overload)": [
        ("maker", "Mitsubishi Electric", r"^MSO?-T\d{2,3}[A-Z]*", "MSO-T10", "high"),
    ],
    "Thermal overload relay": [
        ("maker", "Mitsubishi Electric", r"^TH-T\d{2,3}[A-Z]*", "TH-T18", "high"),
    ],
    "General-purpose plug-in relay": [
        ("maker", "OMRON", r"^MY[1-4][A-Z]*(-[A-Z0-9]+)*", "MY2N-GS DC24", "high"),
        ("maker", "IDEC", r"^R[JU][1-4][A-Z]+(-[A-Z0-9]+)*", "RU2S-D24", "medium"),
    ],
    "Timer relay": [
        ("maker", "OMRON", r"^H3[A-Z]{1,3}-[A-Z0-9-]+", "H3CR-A8", "high"),
    ],
    "Temperature controller": [
        ("maker", "OMRON", r"^E5[A-Z]{1,2}-[A-Z0-9-]+", "E5CC-RX2ASM-800", "high"),
    ],
    "Inductive proximity sensor": [
        ("maker", "OMRON", r"^E2E-[A-Z0-9-]+", "E2E-X5ME1", "high"),
    ],
    "Photoelectric sensor": [
        ("maker", "OMRON", r"^E3Z-[A-Z0-9-]+", "E3Z-T61", "high"),
    ],
    "Limit switch": [
        ("maker", "OMRON", r"^(D4[A-Z]{1,2}|WL[A-Z0-9]+)-?[A-Z0-9-]*", "WLCA2-2", "medium"),
    ],
    "Solid-state relay (SSR)": [
        ("maker", "OMRON", r"^G3[A-Z]{1,3}-[A-Z0-9-]+", "G3NA-210B", "high"),
    ],
    "Switching power supply (DC24V)": [
        ("maker", "OMRON", r"^S8[A-Z]{2}-[A-Z0-9-]+", "S8VK-G06024", "high"),
    ],
    "Inverter (VFD)": [
        ("maker", "Mitsubishi Electric", r"^FR-[A-Z]\d{3}-\d+(\.\d+)?K", "FR-E820-0.75K-1", "high"),
    ],
    "Programmable logic controller": [
        ("maker", "Mitsubishi Electric", r"^FX\d[A-Z]{0,2}-\d{2,3}M[RT]/[A-Z]{2,3}", "FX5U-32MR/ES", "high"),
    ],
    "Push-in fitting": [
        ("maker", "SMC", r"^KQ2[A-Z]{1,2}\d{2}-(M5|\d{2})[A-Z]*", "KQ2H06-01AS", "high"),
    ],
    "Air cylinder": [
        ("maker", "SMC", r"^C[DQ]?[JMQ]2[A-Z]?\d{1,3}-\d{1,4}", "CDJ2B16-50", "medium"),
    ],
    "Deep-groove ball bearing": [
        ("standard", None, r"^(6[0-3]\d{2}|68\d{2}|69\d{2})(ZZ|DDU|LLU|LLB|VV|2RS|2Z|Z|DU|LU)?(C[2-5]|CM)?$", "6205ZZC3", "high"),
    ],
    "Bearing unit (pillow block)": [
        ("standard", None, r"^UC(P|F|FL|FC|T|PH)?\d{3}(D1)?$", "UCP205", "high"),
    ],
    "Tapered roller bearing": [
        ("standard", None, r"^3[0-3]\d{3}[A-Z]*$", "30205", "medium"),
    ],
    "V-belt": [
        ("standard", None, r"^(M|A|B|C|D|E)-?\d{2,3}$", "B-50", "high"),
    ],
    "Narrow V-belt": [
        ("standard", None, r"^(3V|5V|8V|SPZ|SPA|SPB|SPC)-?\d{3,4}$", "5V-750", "medium"),
    ],
    "Roller chain": [
        ("standard", None, r"^RS\d{2,3}(-[1-6])?(-[A-Z]+)?$", "RS40-1", "high"),
    ],
    "O-ring": [
        ("standard", None, r"^(P|G|S|V)\d{1,4}(\.\d)?A?$", "P22A", "high"),
        ("standard", None, r"^AS568-\d{3}$", "AS568-012", "high"),
    ],
    "Indexable insert": [
        ("standard", None, r"^[A-Z]{4}\d{6}(-?[A-Z0-9]{1,4})?$", "CNMG120408-MA", "high"),
    ],
    "Hex bolt": [
        ("standard", None, r"^M\d{1,2}(\.\d)?\s*[×xX＊*]\s*\d{1,3}(L)?$", "M10×30", "high"),
    ],
    "Socket head cap screw": [
        ("standard", None, r"^(CB|CAP)?\s*M\d{1,2}(\.\d)?\s*[×xX＊*-]\s*\d{1,3}$", "CB M6×20", "medium"),
    ],
    "Pan-head machine screw": [
        ("standard", None, r"^M\d(\.\d)?\s*[×xX＊*]\s*\d{1,3}$", "M4×10", "high"),
    ],
    "Threaded rod": [
        ("standard", None, r"^(W3/8|W1/2|M\d{1,2})\s*[×xX]?\s*\d{3,4}$", "W3/8×1000", "medium"),
    ],
    "Covered arc welding electrode": [
        ("standard", None, r"^E\d{4}(-[A-Z0-9]+)?$", "E4313", "medium"),
    ],
    "Solid wire (MAG/CO2)": [
        ("standard", None, r"^YGW1[1-9]$", "YGW11", "high"),
    ],
    "Alkaline battery AA": [("standard", None, r"^LR6$", "LR6", "high")],
    "Alkaline battery AAA": [("standard", None, r"^LR03$", "LR03", "high")],
    "Alkaline battery D": [("standard", None, r"^LR20$", "LR20", "high")],
    "Alkaline battery C": [("standard", None, r"^LR14$", "LR14", "high")],
    "9V battery": [("standard", None, r"^6LR61$", "6LR61", "high")],
    "Lithium coin cell": [("standard", None, r"^(CR|BR)\d{4}$", "CR2032", "high")],
    "Straight LED tube": [
        ("standard", None, r"^LDL(20|40|110)", "LDL40S·N/19/25", "medium"),
    ],
    "Fluorescent tube": [
        ("standard", None, r"^F(L|LR|HF)\d{2,3}", "FL40SSEX-N/37", "medium"),
    ],
    "Tap": [
        ("standard", None, r"^M\d{1,2}(\.\d)?\s*[×xX]\s*P?\d(\.\d{1,2})?$", "M6×1.0", "high"),
    ],
    "Threaded pipe fitting (malleable iron)": [
        ("standard", None, r"^(R|Rc|Rp|G)\s?(1/8|1/4|3/8|1/2|3/4|1|1-1/4|1-1/2|2)$", "Rc1/2", "medium"),
    ],
}

KATAKANA = re.compile(r"^[゠-ヿー・\s]+$")


def slug(text: str) -> str:
    s = re.sub(r"\([^)]*\)", "", text).lower()
    return re.sub(r"[^a-z0-9]+", "-", s).strip("-")


def split(field: str) -> list[str]:
    return [x.strip() for x in field.split(";") if x.strip()]


def build() -> dict:
    items, seen = [], set()
    for line in ROWS.strip().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        parts = line.split("|")
        if len(parts) != 10:
            raise SystemExit(f"bad row ({len(parts)} fields): {line}")
        cat, sub, ja, kana, en, syn, units, packs, attrs, flags = parts
        base = f"{cat[:3]}-{sub}-{slug(en)}"
        iid, n = base, 2
        while iid in seen:
            iid, n = f"{base}-{n}", n + 1
        seen.add(iid)
        if not kana:
            if not KATAKANA.match(ja):
                raise SystemExit(f"kana missing for {ja}")
            kana = ja
        item = {
            "id": iid,
            "category": cat,
            "subcategory": sub,
            "name_ja": ja,
            "name_en": en,
            "kana": kana,
            "synonyms": [s for s in split(syn) if s != ja],
            "units": split(units),
            "typical_packs": split(packs),
            "attributes": split(attrs),
            "mpn_patterns": [
                {k: v for k, v in zip(("scope", "maker", "pattern", "example", "confidence"), p) if v is not None}
                for p in MPN.get(en, [])
            ],
            "uncertain": "u" in flags,
        }
        items.append(item)
    unused = set(MPN) - {i["name_en"] for i in items}
    if unused:
        raise SystemExit(f"MPN keys with no item: {sorted(unused)}")
    return {
        "schema_version": 1,
        "version": "1.0.0",
        "locale": "ja-JP",
        "note": "Hand-written MRO buying dictionary for Japanese factories. '(商標)' marks a trademark used colloquially as a generic name; it is a search synonym, not a brand recommendation. 'uncertain': naming or pack sizes vary widely, confirm with the buyer. typical_packs are common purchase units, not a catalogue.",
        "items": items,
    }


if __name__ == "__main__":
    out = pathlib.Path(__file__).with_name("items.json")
    data = build()
    out.write_text(json.dumps(data, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    c = {}
    for i in data["items"]:
        c[i["category"]] = c.get(i["category"], 0) + 1
    print(len(data["items"]), c)
