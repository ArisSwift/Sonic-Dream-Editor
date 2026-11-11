// Temporary diagnostic: does this XML build a window?
//
//   cargo run -p sdt-editor --example xmlcheck -- /tmp/x.xml

use raven::*;

fn main() {
    let path = std::env::args().nth(1).expect("usage: xmlcheck <file.xml>");
    let xml = std::fs::read_to_string(&path).expect("read xml");
    unsafe {
        let mut desc: RInitDesc = std::mem::zeroed();
        desc.backend = if std::env::var("NULL").is_ok() { R_BACKEND_NULL } else { R_BACKEND_VULKAN };
        desc.headless = std::env::var("NULL").is_ok() as i32;
        assert_ne!(Rinit_ex(&desc), 0, "cannot start Raven");
        let window = window_from_xml(&xml);
        println!("window: {window}");
        let mut error: RXmlError = std::mem::zeroed();
        let node = Rxml_document_parse(xml.as_ptr() as *const _, xml.len(), &mut error);
        println!("parse : {node} (line {}) {}", error.line, text(error.message.as_ptr()));
        Rshutdown();
    }
}
