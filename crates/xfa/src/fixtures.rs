//! Synthetic fixtures for tests in this and other crates (never real forms).

#![doc(hidden)]

/// A small dynamic form: a letter page master with a page-number draw, a title, a text field
/// with a caption on top, a check box, a radio group beside a question, a date field, a table
/// with a repeated header row and `rows` data rows, and a last section that starts a new page.
pub fn template(rows: usize) -> String {
    let mut t = String::from(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<config xmlns="http://www.xfa.org/schema/xci/3.0/"><present><pdf><version>1.7</version></pdf></present><template><base/></template></config>
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb" locale="en_US">
 <pageSet>
  <pageArea name="front" id="front"><occur min="1" max="1"/>
   <contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/>
   <medium stock="letter" short="8.5in" long="11in"/>
   <draw name="pn" x="7in" y="10.6in" w="1in" h="0.2in"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>Page <span xfa:embed="#pageNo" xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"/> of <span xfa:embed="#pageCount" xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"/></p></body></exData></value><font typeface="Arial" size="8pt"/></draw>
   <field name="pageNo" id="pageNo" presence="hidden" w="1in" h="0.2in"><ui><textEdit/></ui><event activity="ready" ref="$layout"><script contentType="application/x-javascript">this.rawValue = xfa.layout.page(this);</script></event></field>
   <field name="pageCount" id="pageCount" presence="hidden" w="1in" h="0.2in"><ui><textEdit/></ui><event activity="ready" ref="$layout"><script contentType="application/x-javascript">this.rawValue = xfa.layout.pageCount();</script></event></field>
  </pageArea>
  <pageArea name="rest"><occur max="-1"/>
   <contentArea x="0.5in" y="1in" w="7.5in" h="9.5in"/>
   <medium stock="letter" short="8.5in" long="11in"/>
  </pageArea>
 </pageSet>
 <subform name="head" layout="lr-tb">
  <draw name="title" w="7.5in" h="0.4in"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p style="font-weight:bold">Sample Form</p><p>Second line</p></body></exData></value><font typeface="Arial" size="12pt"/><para hAlign="center"/></draw>
  <draw name="hidden" presence="hidden" w="7.5in" h="3in"><value><text>never shown</text></value></draw>
  <field name="familyName" w="3in" h="0.5in"><ui><textEdit><border><edge presence="hidden"/><edge presence="hidden"/><edge/><edge presence="hidden"/></border></textEdit></ui><font typeface="Courier New" size="9pt"/><caption placement="top" reserve="0.2in"><value><text>Family name</text></value><font typeface="Arial" size="7pt"/></caption><assist><toolTip>Your family name</toolTip></assist><value><text maxChars="30"/></value></field>
  <field name="agree" w="2in" h="0.25in"><ui><checkButton><border><edge/><fill/></border></checkButton></ui><caption placement="right" reserve="1.5in"><value><text>I agree</text></value></caption><items><integer>1</integer><integer>0</integer></items></field>
  <field name="born" w="2in" h="0.5in"><ui><dateTimeEdit/><picture>date{YYYY-MM-DD}</picture></ui><caption placement="top" reserve="0.2in"><value><text>Date of birth</text></value></caption><value><date/></value></field>
  <draw name="q" w="6.5in" h="0.25in"><value><text>Have you ever?</text></value></draw>
  <exclGroup name="answer" layout="lr-tb">
   <field name="yes" w="0.5in" h="0.25in"><ui><checkButton/></ui><items><text>Y</text></items></field>
   <field name="no" w="0.5in" h="0.25in"><ui><checkButton/></ui><items><text>N</text></items></field>
  </exclGroup>
  <field name="go" w="1in" h="0.3in"><ui><button/></ui><caption><value><text>Reset</text></value></caption><border><edge stroke="raised"/><fill><color value="212,208,200"/></fill></border><event activity="click"><script contentType="application/x-javascript">xfa.host.resetData();</script></event></field>
 </subform>
 <subform name="table" layout="table" columnWidths="2in 3in 2.5in">
  <overflow leader="header"/>
  <subform name="header" layout="row"><occur max="-1"/>
   <draw w="2in" h="0.3in"><value><text>From</text></value><border><edge/><fill><color value="200,200,200"/></fill></border></draw>
   <draw h="0.3in"><value><text>Activity</text></value><border><edge/></border></draw>
   <draw h="0.3in"><value><text>Place</text></value><border><edge/></border></draw>
  </subform>
"##,
    );
    for i in 0..rows {
        t.push_str(&format!(
            r##"  <subform name="row" layout="row"><occur max="-1"/><field name="from{i}" h="0.4in"><ui><textEdit/></ui></field><field name="what{i}" h="0.4in"><ui><textEdit/></ui></field><field name="where{i}" h="0.4in"><ui><textEdit/></ui></field></subform>
"##
        ));
    }
    t.push_str(
        r##" </subform>
 <subform name="last" layout="tb"><breakBefore targetType="pageArea"/>
  <draw name="sig" w="7.5in" h="0.3in"><value><text>Signature</text></value></draw>
 </subform>
</subform>
</template>
</xdp:xdp>
"##,
    );
    t
}

/// [`template`] with a datasets packet holding `data` (the children of `xfa:data`).
pub fn template_with_data(rows: usize, data: &str) -> String {
    let t = template(rows);
    let packet =
        format!("<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data>{data}</xfa:data></xfa:datasets>\n</xdp:xdp>");
    t.replace("</xdp:xdp>", &packet)
}

/// A static XFA form: an AcroForm with Designer-style field names (`form1[0].page1[0].name[0]`,
/// a text field, and `…agree[0]`, a check box with on state `1`), plus XFA packets whose
/// datasets hold `data` (the children of `xfa:data`). No `/NeedsRendering`.
pub fn static_shell(data: &str) -> Vec<u8> {
    let xdp = format!(
        "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><template xmlns=\"http://www.xfa.org/schema/xfa-template/3.3/\"><subform name=\"form1\"/></template><xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data>{data}</xfa:data></xfa:datasets></xdp:xdp>"
    );
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /XFA 5 0 R /Fields [6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> >> >> >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Annots [8 0 R 9 0 R] >>".to_vec(),
        b"<< /Length 0 >>\nstream\n\nendstream".to_vec(),
        format!("<< /Length {} >>\nstream\n{xdp}\nendstream", xdp.len()).into_bytes(),
        b"<< /T (form1[0]) /Kids [7 0 R] >>".to_vec(),
        b"<< /T (page1[0]) /Parent 6 0 R /Kids [8 0 R 9 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name[0]) /Parent 7 0 R /Rect [72 700 300 720] /P 3 0 R /DA (/Helv 10 Tf 0 g) /MK << /BC [0 0 0] >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree[0]) /Parent 7 0 R /Rect [72 660 84 672] /P 3 0 R /V /Off /AS /Off /MK << /BC [0 0 0] >> /AP << /N << /1 10 0 R /Off 10 0 R >> >> >>".to_vec(),
        b"<< /Type /XObject /Subtype /Form /BBox [0 0 12 12] /Length 0 >>\nstream\n\nendstream".to_vec(),
    ];
    assemble(objs)
}

fn assemble(objs: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A scripted form (JavaScript events): `qty` (initialised to 2, validated ≤ 100) × `price`
/// (5) = `total` (calculated); a check box `more` that shows the hidden `details` subform;
/// a table whose `row` (item, amount) repeats, with `addRow`, `removeRow` and `hello` buttons;
/// `grand`, the calculated sum of the amounts.
pub fn scripted_template() -> String {
    r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="page1" layout="tb">
  <event activity="initialize"><script contentType="application/x-javascript">if (qty.rawValue === null) qty.rawValue = 2;</script></event>
  <field name="qty" w="2in" h="0.3in"><ui><numericEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Qty</text></value></caption>
   <validate><script contentType="application/x-javascript">this.rawValue === null || this.rawValue &lt;= 100</script><message><text name="scriptTest">Quantity must be 100 or less</text></message></validate></field>
  <field name="price" w="2in" h="0.3in"><ui><numericEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Price</text></value></caption><value><integer>5</integer></value></field>
  <field name="total" w="2in" h="0.3in" access="readOnly"><ui><numericEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Total</text></value></caption>
   <calculate><script contentType="application/x-javascript">qty.rawValue * price.rawValue</script></calculate></field>
  <field name="more" w="2in" h="0.3in"><ui><checkButton/></ui><caption placement="right" reserve="1.5in"><value><text>More details</text></value></caption><items><integer>1</integer><integer>0</integer></items>
   <event activity="change"><script contentType="application/x-javascript">details.presence = (this.rawValue == 1) ? "visible" : "hidden";</script></event></field>
  <subform name="details" layout="tb" presence="hidden">
   <field name="note" w="4in" h="0.3in"><ui><textEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Note</text></value></caption></field>
  </subform>
  <subform name="table" layout="table" columnWidths="3in 2in">
   <subform name="row" layout="row"><occur min="1" max="-1"/>
    <field name="item" h="0.3in"><ui><textEdit/></ui></field>
    <field name="amount" h="0.3in"><ui><numericEdit/></ui></field>
   </subform>
  </subform>
  <field name="grand" w="2in" h="0.3in" access="readOnly"><ui><numericEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Sum</text></value></caption>
   <calculate><script contentType="application/x-javascript">var t = 0; var rows = xfa.resolveNodes("table.row[*]"); for (var i = 0; i &lt; rows.length; i++) { t += (rows.item(i).amount.rawValue || 0); } t</script></calculate></field>
  <field name="addRow" w="1in" h="0.3in"><ui><button/></ui><caption><value><text>Add row</text></value></caption>
   <event activity="click"><script contentType="application/x-javascript">table._row.addInstance(1);</script></event></field>
  <field name="removeRow" w="1in" h="0.3in"><ui><button/></ui><caption><value><text>Remove row</text></value></caption>
   <event activity="click"><script contentType="application/x-javascript">if (table._row.count > 1) { table._row.removeInstance(table._row.count - 1); }</script></event></field>
  <field name="hello" w="1in" h="0.3in"><ui><button/></ui><caption><value><text>Hello</text></value></caption>
   <event activity="click"><script contentType="application/x-javascript">xfa.host.messageBox("Hello " + qty.rawValue);</script></event></field>
  <field name="legacy" w="1in" h="0.3in"><ui><button/></ui><caption><value><text>Legacy</text></value></caption>
   <event activity="click"><script>$host.messageBox("FormCalc")</script></event></field>
 </subform>
</subform>
</template>
</xdp:xdp>
"##
    .to_string()
}

/// [`scripted_template`] with every script in FormCalc (no `contentType`, as Designer writes
/// the default language): the same fields, plus `words`, the total in English words.
pub fn formcalc_template() -> String {
    r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="page1" layout="tb">
  <event activity="initialize"><script>if (HasValue(qty) == 0) then qty = 2 endif</script></event>
  <field name="qty" w="2in" h="0.3in"><ui><numericEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Qty</text></value></caption>
   <validate><script>HasValue($) == 0 or $ &lt;= 100</script><message><text name="scriptTest">Quantity must be 100 or less</text></message></validate></field>
  <field name="price" w="2in" h="0.3in"><ui><numericEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Price</text></value></caption><value><integer>5</integer></value></field>
  <field name="total" w="2in" h="0.3in" access="readOnly"><ui><numericEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Total</text></value></caption>
   <calculate><script>qty * price</script></calculate></field>
  <field name="words" w="4in" h="0.3in" access="readOnly"><ui><textEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Words</text></value></caption>
   <calculate><script>WordNum(total)</script></calculate></field>
  <field name="more" w="2in" h="0.3in"><ui><checkButton/></ui><caption placement="right" reserve="1.5in"><value><text>More details</text></value></caption><items><integer>1</integer><integer>0</integer></items>
   <event activity="change"><script>if ($ == 1) then
  details.presence = "visible"
else
  details.presence = "hidden"
endif</script></event></field>
  <subform name="details" layout="tb" presence="hidden">
   <field name="note" w="4in" h="0.3in"><ui><textEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Note</text></value></caption></field>
  </subform>
  <subform name="table" layout="table" columnWidths="3in 2in">
   <subform name="row" layout="row"><occur min="1" max="-1"/>
    <field name="item" h="0.3in"><ui><textEdit/></ui></field>
    <field name="amount" h="0.3in"><ui><numericEdit/></ui></field>
   </subform>
  </subform>
  <field name="grand" w="2in" h="0.3in" access="readOnly"><ui><numericEdit/></ui><caption placement="left" reserve="0.8in"><value><text>Sum</text></value></caption>
   <calculate><script>Sum(table.row[*].amount)</script></calculate></field>
  <field name="addRow" w="1in" h="0.3in"><ui><button/></ui><caption><value><text>Add row</text></value></caption>
   <event activity="click"><script>table._row.addInstance(1)</script></event></field>
  <field name="removeRow" w="1in" h="0.3in"><ui><button/></ui><caption><value><text>Remove row</text></value></caption>
   <event activity="click"><script>if (table._row.count > 1) then table._row.removeInstance(table._row.count - 1) endif</script></event></field>
  <field name="hello" w="1in" h="0.3in"><ui><button/></ui><caption><value><text>Hello</text></value></caption>
   <event activity="click"><script>$host.messageBox(Concat("Hello ", qty))</script></event></field>
 </subform>
</subform>
</template>
</xdp:xdp>
"##
    .to_string()
}

/// Shaped like a real Designer leave-request form: a positioned page whose unnamed subform holds
/// an area (`box`) with a top-to-bottom table of positioned rows (`<occur min="3"/>`) and a
/// read-only total that a FormCalc calculate works out over a fixed range of rows, as such forms
/// do. `data` is `xfa:data` content as Adobe's viewers write it: areas are not data scopes, so
/// there is no `box` element.
pub fn leave_template(data: &str) -> String {
    let datasets = if data.is_empty() {
        String::new()
    } else {
        format!("<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data>{data}</xfa:data></xfa:datasets>")
    };
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="page" w="7.5in" h="9in">
  <subform w="7.5in" h="8in">
   <area name="box" x="0.25in" y="0.5in">
    <subform name="table" layout="tb" w="5in">
     <subform name="row" w="5in" h="0.3in"><occur min="3" max="-1"/>
      <field name="reason" x="0in" w="3in" h="0.3in"><ui><textEdit/></ui></field>
      <field name="days" x="3in" w="1in" h="0.3in"><ui><numericEdit/></ui></field>
     </subform>
    </subform>
    <field name="carried" y="1.5in" w="2in" h="0.3in"><ui><numericEdit/></ui></field>
    <field name="allowance" y="2in" w="2in" h="0.3in"><ui><numericEdit/></ui><value><float>30</float></value></field>
    <field name="rest" y="2.5in" w="2in" h="0.3in" access="readOnly"><ui><numericEdit/></ui>
     <calculate><script>
      var left = carried + allowance
      for i=0 upto 2 step 1 do
       if (table.row[i].reason == "Regular") then
        left = left - table.row[i].days
       endif
      endfor
      this = left
     </script></calculate></field>
   </area>
  </subform>
 </subform>
</subform>
</template>
{datasets}
</xdp:xdp>
"##
    )
}

/// A 2 × 2 RGBA PNG: red, green / blue, transparent.
pub fn tiny_png() -> Vec<u8> {
    TINY_PNG.to_vec()
}

const TINY_PNG: &[u8] = b"\x89\x50\x4e\x47\x0d\x0a\x1a\x0a\x00\x00\x00\x0d\x49\x48\x44\x52\x00\x00\x00\x02\x00\x00\x00\x02\x08\x06\x00\x00\x00\x72\xb6\x0d\x24\x00\x00\x00\x13\x49\x44\x41\x54\x78\xda\x63\xf8\xcf\xc0\xf0\x1f\x0c\x81\x34\x88\x60\x00\x00\x3f\xd2\x05\xfb\x7f\xe6\x6a\x2b\x00\x00\x00\x00\x49\x45\x4e\x44\xae\x42\x60\x82";

/// A 1 × 1 white GIF.
pub const TINY_GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

/// A form with the other field kinds: `country`, a drop-down whose shown names save as codes
/// (`Canada`/`CA`, `France`/`FR`, `Japan`/`JP`; default `FR`); `langs`, a multi-select list
/// box (`English`, `French`, `Spanish`); `other`, an editable drop-down (`A`, `B`); `pin`, a
/// password field; `sign`, a signature field; `photo`, an image field whose template picture
/// is [`tiny_png`]; `code`, a barcode field with value `12345`; `logo`, a draw showing
/// [`TINY_GIF`]; `summary`, calculated (JavaScript) from `country`. `data` is the children of
/// `xfa:data` (empty for none).
pub fn fields_template(data: &str) -> String {
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD.encode(tiny_png());
    let gif = base64::engine::general_purpose::STANDARD.encode(TINY_GIF);
    let datasets = if data.is_empty() {
        String::new()
    } else {
        format!("<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data>{data}</xfa:data></xfa:datasets>")
    };
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="page1" layout="tb">
  <field name="country" w="3in" h="0.3in"><ui><choiceList/></ui><caption placement="left" reserve="1in"><value><text>Country</text></value></caption>
   <items><text>Canada</text><text>France</text><text>Japan</text></items>
   <items save="1" presence="hidden"><text>CA</text><text>FR</text><text>JP</text></items>
   <value><text>FR</text></value></field>
  <field name="langs" w="3in" h="0.8in"><ui><choiceList open="multiSelect"/></ui><caption placement="left" reserve="1in"><value><text>Languages</text></value></caption>
   <items><text>English</text><text>French</text><text>Spanish</text></items></field>
  <field name="other" w="3in" h="0.3in"><ui><choiceList open="onEntry" textEntry="1"/></ui><items><text>A</text><text>B</text></items></field>
  <field name="pin" w="2in" h="0.3in"><ui><passwordEdit/></ui><caption placement="left" reserve="0.5in"><value><text>PIN</text></value></caption></field>
  <field name="sign" w="3in" h="0.6in"><ui><signature/></ui><caption placement="top" reserve="0.2in"><value><text>Sign here</text></value></caption><border><edge/></border></field>
  <field name="photo" w="1in" h="1in"><ui><imageEdit/></ui><value><image contentType="image/png">{png}</image></value></field>
  <field name="code" w="2in" h="0.5in"><ui><barcode type="code128A"/></ui><value><text>12345</text></value><border><edge/></border></field>
  <draw name="logo" w="0.5in" h="0.5in"><value><image contentType="image/gif">{gif}</image></value></draw>
  <field name="summary" w="3in" h="0.3in" access="readOnly"><ui><textEdit/></ui>
   <calculate><script contentType="application/x-javascript">"Country: " + (country.rawValue || "none")</script></calculate></field>
 </subform>
</subform>
</template>
{datasets}
</xdp:xdp>
"##
    )
}

/// A PDF shell around `xdp`: one placeholder page, `/NeedsRendering true`, no fields.
pub fn shell(xdp: &str) -> Vec<u8> {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm << /XFA 5 0 R /Fields [] >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
        b"<< /Length 44 >>\nstream\nBT /F1 12 Tf 72 700 Td (Please wait...) Tj ET\nendstream".to_vec(),
        format!("<< /Length {} >>\nstream\n{xdp}\nendstream", xdp.len()).into_bytes(),
    ];
    assemble(objs)
}
