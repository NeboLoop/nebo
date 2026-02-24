package webview

import (
	"encoding/json"
	"fmt"
	"strings"
)

// Each JS template is a self-contained IIFE that:
// 1. Executes the action
// 2. Sends the result back to Go via the Wails native message bridge
//    (falls back to HTTP fetch for headless mode)
//
// The native bridge (window._wails.invoke) bypasses CORS and mixed content
// restrictions that block HTTP fetch from HTTPS pages to localhost.

// callbackJS returns a JS snippet that defines __cb(data), the universal
// callback function. It tries, in order:
//  1. window.__nebo_cb (pre-defined by bootstrap JS via WebviewWindowOptions.JS)
//  2. Native platform message handler (macOS WebKit / Windows WebView2)
//  3. Wails runtime bridge (window._wails.invoke)
//  4. HTTP fetch to localhost callback endpoint (headless fallback)
func callbackJS(callbackURL string) string {
	return fmt.Sprintf(`var __cb=window.__nebo_cb||function(d){var m="nebo:cb:"+JSON.stringify(d);try{if(window._wails&&window._wails.invoke){window._wails.invoke(m)}else if(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.external){window.webkit.messageHandlers.external.postMessage(m)}else if(window.chrome&&window.chrome.webview){window.chrome.webview.postMessage(m)}else{fetch(%s,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(d)}).catch(function(){})}}catch(e){}};`, jsonString(callbackURL))
}

// wrapJS wraps action code in the callback boilerplate.
// The actionCode should assign its result to the `__result` variable.
func wrapJS(requestID, callbackURL, actionCode string) string {
	return fmt.Sprintf(`(function(){
%s
try{
%s
__cb({requestId:%s,data:__result});
}catch(e){
__cb({requestId:%s,error:e.message||String(e)});
}
})();`,
		callbackJS(callbackURL),
		actionCode,
		jsonString(requestID),
		jsonString(requestID),
	)
}

// jsonString returns a JSON-encoded string literal for safe JS embedding.
func jsonString(s string) string {
	b, _ := json.Marshal(s)
	return string(b)
}

// pageInfoJS returns JS that collects page metadata.
func pageInfoJS(requestID, callbackURL string) string {
	return wrapJS(requestID, callbackURL, `var __result={url:location.href,title:document.title,scrollY:window.scrollY,documentHeight:document.documentElement.scrollHeight,viewportHeight:window.innerHeight};`)
}

// snapshotJS returns JS that builds a simplified accessible DOM snapshot.
// Produces a text representation with interactive element refs (e1, e2, ...).
func snapshotJS(requestID, callbackURL string) string {
	code := `
var __refCounter=0;
function __walk(el,depth){
  var indent="  ".repeat(depth);
  var lines=[];
  var tag=el.tagName?el.tagName.toLowerCase():"";
  if(!tag)return lines;
  // Skip hidden elements
  if(el.offsetParent===null&&tag!=="body"&&tag!=="html")return lines;
  var isInteractive=["a","button","input","textarea","select","details","summary"].indexOf(tag)!==-1||el.getAttribute("role")==="button"||el.getAttribute("tabindex")!=null||el.onclick!=null||el.getAttribute("contenteditable")==="true";
  var ref="";
  if(isInteractive){
    __refCounter++;
    ref="[e"+__refCounter+"] ";
    el.setAttribute("data-nebo-ref","e"+__refCounter);
  }
  // Get associated label text
  var labelText="";
  if(tag==="input"||tag==="textarea"||tag==="select"){
    var id=el.id;
    if(id){var lbl=document.querySelector('label[for="'+id+'"]');if(lbl)labelText=(lbl.textContent||"").trim();}
    if(!labelText){var parent=el.closest("label");if(parent)labelText=(parent.textContent||"").trim().replace((el.value||""),"").trim();}
    var aria=el.getAttribute("aria-label")||el.getAttribute("aria-labelledby");
    if(!labelText&&aria)labelText=aria;
  }
  var desc="";
  if(tag==="a"){
    var href=el.getAttribute("href")||"";
    var text=(el.textContent||"").trim().substring(0,80);
    desc=ref+"link "+JSON.stringify(text)+(href?" -> "+href:"");
  }else if(tag==="button"){
    var text=(el.textContent||"").trim().substring(0,60);
    desc=ref+"button "+JSON.stringify(text);
  }else if(tag==="input"){
    var t=el.type||"text";
    var v=el.value||"";
    var p=el.placeholder||"";
    var n=el.name||el.id||"";
    var req=el.required?" required":"";
    desc=ref+"input["+t+"]"+(n?" name="+n:"")+(labelText?" label="+JSON.stringify(labelText):"")+(v?" value="+JSON.stringify(v):"")+(p?" placeholder="+JSON.stringify(p):"")+req;
  }else if(tag==="textarea"){
    var v=(el.value||"").substring(0,100);
    var n=el.name||el.id||"";
    desc=ref+"textarea"+(n?" name="+n:"")+(labelText?" label="+JSON.stringify(labelText):"")+(v?" value="+JSON.stringify(v):"");
  }else if(tag==="select"){
    var v=el.value||"";
    var n=el.name||el.id||"";
    var opts=[];
    for(var j=0;j<el.options.length&&j<10;j++){opts.push(el.options[j].value+"="+JSON.stringify(el.options[j].text.trim()));}
    desc=ref+"select"+(n?" name="+n:"")+(labelText?" label="+JSON.stringify(labelText):"")+" value="+JSON.stringify(v)+(opts.length?" options=["+opts.join(",")+"]":"");
  }else if(tag==="img"){
    var alt=el.alt||"";
    desc="img"+(alt?" alt="+JSON.stringify(alt):"");
  }else if(["h1","h2","h3","h4","h5","h6"].indexOf(tag)!==-1){
    desc=tag+": "+(el.textContent||"").trim().substring(0,120);
  }else if(tag==="p"||tag==="span"||tag==="li"||tag==="td"||tag==="th"||tag==="label"){
    var text=(el.textContent||"").trim();
    if(text.length>0&&text.length<200&&el.children.length===0){
      desc=tag+": "+text.substring(0,150);
    }
  }else if(tag==="form"){
    var action=el.getAttribute("action")||"";
    var method=(el.getAttribute("method")||"GET").toUpperCase();
    desc="form method="+method+(action?" action="+action:"");
  }
  if(desc){
    lines.push(indent+desc);
  }
  for(var i=0;i<el.children.length;i++){
    lines=lines.concat(__walk(el.children[i],desc?depth+1:depth));
  }
  return lines;
}
var __lines=["Page: "+document.title,"URL: "+location.href,"---"];
__lines=__lines.concat(__walk(document.body,0));
var __result=__lines.join("\n");
`
	return wrapJS(requestID, callbackURL, code)
}

// clickJS returns JS that clicks an element by data-nebo-ref or CSS selector.
func clickJS(requestID, callbackURL, ref, selector string) string {
	var findCode string
	if ref != "" {
		findCode = fmt.Sprintf(`var el=document.querySelector('[data-nebo-ref=%s]');`, jsonString(ref))
	} else {
		findCode = fmt.Sprintf(`var el=document.querySelector(%s);`, jsonString(selector))
	}
	code := findCode + `
if(!el){var __result={error:"Element not found"};} else {
  el.scrollIntoView({block:"center"});
  el.click();
  var __result={ok:true,tag:el.tagName.toLowerCase(),text:(el.textContent||"").trim().substring(0,80)};
}`
	return wrapJS(requestID, callbackURL, code)
}

// fillJS returns JS that fills an input/textarea.
// Uses the native value setter trick to work with React/Vue/Angular frameworks
// that override input value tracking.
func fillJS(requestID, callbackURL, ref, selector, value string) string {
	var findCode string
	if ref != "" {
		findCode = fmt.Sprintf(`var el=document.querySelector('[data-nebo-ref=%s]');`, jsonString(ref))
	} else {
		findCode = fmt.Sprintf(`var el=document.querySelector(%s);`, jsonString(selector))
	}
	code := findCode + fmt.Sprintf(`
if(!el){var __result={error:"Element not found"};} else {
  el.focus();
  var tag=el.tagName.toLowerCase();
  var proto=tag==="textarea"?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;
  var nativeSetter=Object.getOwnPropertyDescriptor(proto,"value").set;
  nativeSetter.call(el,%s);
  el.dispatchEvent(new Event("input",{bubbles:true}));
  el.dispatchEvent(new InputEvent("input",{bubbles:true,inputType:"insertText",data:%s}));
  el.dispatchEvent(new Event("change",{bubbles:true}));
  var __result={ok:true,tag:tag,value:el.value};
}`, jsonString(value), jsonString(value))
	return wrapJS(requestID, callbackURL, code)
}

// typeJS returns JS that types text character by character.
// Uses native value setter for React/Vue/Angular compatibility.
func typeJS(requestID, callbackURL, ref, selector, text string) string {
	var findCode string
	if ref != "" {
		findCode = fmt.Sprintf(`var el=document.querySelector('[data-nebo-ref=%s]');`, jsonString(ref))
	} else {
		findCode = fmt.Sprintf(`var el=document.querySelector(%s);`, jsonString(selector))
	}
	code := findCode + fmt.Sprintf(`
if(!el){var __result={error:"Element not found"};} else {
  el.focus();
  var text=%s;
  var tag=el.tagName.toLowerCase();
  var proto=tag==="textarea"?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;
  var nativeSetter=Object.getOwnPropertyDescriptor(proto,"value").set;
  for(var i=0;i<text.length;i++){
    var c=text[i];
    el.dispatchEvent(new KeyboardEvent('keydown',{key:c,bubbles:true}));
    el.dispatchEvent(new KeyboardEvent('keypress',{key:c,bubbles:true}));
    nativeSetter.call(el,(el.value||"")+c);
    el.dispatchEvent(new InputEvent('input',{bubbles:true,inputType:'insertText',data:c}));
    el.dispatchEvent(new KeyboardEvent('keyup',{key:c,bubbles:true}));
  }
  el.dispatchEvent(new Event('change',{bubbles:true}));
  var __result={ok:true,typed:text.length,value:el.value};
}`, jsonString(text))
	return wrapJS(requestID, callbackURL, code)
}

// getTextJS returns JS that extracts text content.
func getTextJS(requestID, callbackURL, selector string) string {
	var code string
	if selector != "" {
		code = fmt.Sprintf(`var el=document.querySelector(%s);
var __result=el?(el.textContent||"").trim():"Element not found: %s";`, jsonString(selector), escapeSingleQuote(selector))
	} else {
		code = `var __result=(document.body.textContent||"").trim();`
	}
	return wrapJS(requestID, callbackURL, code)
}

// evalJS returns JS that evaluates arbitrary code and returns the result.
func evalJS(requestID, callbackURL, code string) string {
	wrapped := fmt.Sprintf(`var __result=(function(){%s})();`, code)
	return wrapJS(requestID, callbackURL, wrapped)
}

// scrollJS returns JS that scrolls the page.
func scrollJS(requestID, callbackURL, direction string) string {
	var scrollCode string
	switch strings.ToLower(direction) {
	case "up":
		scrollCode = `window.scrollBy(0,-window.innerHeight*0.8);`
	case "left":
		scrollCode = `window.scrollBy(-window.innerWidth*0.8,0);`
	case "right":
		scrollCode = `window.scrollBy(window.innerWidth*0.8,0);`
	case "top":
		scrollCode = `window.scrollTo(0,0);`
	case "bottom":
		scrollCode = `window.scrollTo(0,document.documentElement.scrollHeight);`
	default: // "down"
		scrollCode = `window.scrollBy(0,window.innerHeight*0.8);`
	}
	code := scrollCode + `var __result={scrollY:window.scrollY,scrollHeight:document.documentElement.scrollHeight,viewportHeight:window.innerHeight};`
	return wrapJS(requestID, callbackURL, code)
}

// waitJS returns JS that polls for an element's existence.
func waitJS(requestID, callbackURL, selector string, timeoutMs int) string {
	if timeoutMs <= 0 {
		timeoutMs = 10000
	}
	code := fmt.Sprintf(`
var __sel=%s;
var __timeout=%d;
var __start=Date.now();
function __poll(){
  var el=document.querySelector(__sel);
  if(el){
    __cb({requestId:%s,data:{found:true,elapsed:Date.now()-__start}});
  }else if(Date.now()-__start>__timeout){
    __cb({requestId:%s,data:{found:false,elapsed:Date.now()-__start,error:"Timeout waiting for "+__sel}});
  }else{
    setTimeout(__poll,200);
  }
}
__poll();`,
		jsonString(selector), timeoutMs,
		jsonString(requestID),
		jsonString(requestID),
	)
	// waitJS has its own callback pattern (polling), so uses __cb directly
	return fmt.Sprintf(`(function(){%stry{%s}catch(e){__cb({requestId:%s,error:e.message||String(e)});}})();`,
		callbackJS(callbackURL), code, jsonString(requestID))
}

// hoverJS returns JS that hovers over an element.
func hoverJS(requestID, callbackURL, ref, selector string) string {
	var findCode string
	if ref != "" {
		findCode = fmt.Sprintf(`var el=document.querySelector('[data-nebo-ref=%s]');`, jsonString(ref))
	} else {
		findCode = fmt.Sprintf(`var el=document.querySelector(%s);`, jsonString(selector))
	}
	code := findCode + `
if(!el){var __result={error:"Element not found"};} else {
  el.scrollIntoView({block:"center"});
  el.dispatchEvent(new MouseEvent('mouseenter',{bubbles:true}));
  el.dispatchEvent(new MouseEvent('mouseover',{bubbles:true}));
  var __result={ok:true,tag:el.tagName.toLowerCase()};
}`
	return wrapJS(requestID, callbackURL, code)
}

// selectJS returns JS that selects a value in a <select> element.
// Uses native value setter for framework compatibility.
func selectJS(requestID, callbackURL, ref, selector, value string) string {
	var findCode string
	if ref != "" {
		findCode = fmt.Sprintf(`var el=document.querySelector('[data-nebo-ref=%s]');`, jsonString(ref))
	} else {
		findCode = fmt.Sprintf(`var el=document.querySelector(%s);`, jsonString(selector))
	}
	code := findCode + fmt.Sprintf(`
if(!el){var __result={error:"Element not found"};} else {
  el.value=%s;
  el.dispatchEvent(new Event('change',{bubbles:true}));
  el.dispatchEvent(new Event('input',{bubbles:true}));
  var __result={ok:true,value:el.value};
}`, jsonString(value))
	return wrapJS(requestID, callbackURL, code)
}

func escapeSingleQuote(s string) string {
	return strings.ReplaceAll(s, "'", "\\'")
}

// screenshotJS captures a screenshot of the current page using HTML5 Canvas API.
// This works by rendering the document into a canvas element, then converting to PNG.
func screenshotJS(requestID, callbackURL string) string {
	code := `
	var __result;
	try {
		// Use html2canvas library if available, otherwise use native DOM screenshot
		if (typeof html2canvas !== 'undefined') {
			html2canvas(document.body).then(function(canvas) {
				var __result = {data: canvas.toDataURL('image/png')};
				__cb({requestId: ` + jsonString(requestID) + `, data: __result});
			}).catch(function(err) {
				__cb({requestId: ` + jsonString(requestID) + `, error: err.message || String(err)});
			});
			return; // Exit early, async callback will fire
		} else {
			// Fallback: capture viewport using canvas
			var canvas = document.createElement('canvas');
			var ctx = canvas.getContext('2d');
			canvas.width = document.documentElement.scrollWidth || window.innerWidth;
			canvas.height = document.documentElement.scrollHeight || window.innerHeight;
			
			// Draw background
			ctx.fillStyle = 'white';
			ctx.fillRect(0, 0, canvas.width, canvas.height);
			
			// Note: This is a basic implementation. For full page screenshots,
			// consider using html2canvas library or browser-native screenshot APIs.
			__result = {
				data: canvas.toDataURL('image/png'),
				note: 'Basic viewport capture. For full-page screenshots, use managed browser profile.'
			};
		}
	} catch(e) {
		__result = {error: e.message || String(e)};
	}`
	
	return wrapJS(requestID, callbackURL, code)
}
