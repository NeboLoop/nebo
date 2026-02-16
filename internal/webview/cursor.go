package webview

import (
	"fmt"
	"math/rand"
)

// cursorClickJS returns JS that simulates a realistic mouse movement path
// from a random starting point to the target element, then clicks.
// Uses quadratic bezier curves with jitter for human-like movement.
func cursorClickJS(requestID, callbackURL, ref, selector string) string {
	var findCode string
	if ref != "" {
		findCode = fmt.Sprintf(`var el=document.querySelector('[data-nebo-ref=%s]');`, jsonString(ref))
	} else {
		findCode = fmt.Sprintf(`var el=document.querySelector(%s);`, jsonString(selector))
	}

	// Randomize movement parameters for each call
	steps := 15 + rand.Intn(20)    // 15-34 steps
	jitter := 1 + rand.Intn(3)     // 1-3px jitter per step
	baseDelay := 5 + rand.Intn(10) // 5-14ms base delay between steps

	code := findCode + fmt.Sprintf(`
if(!el){var __result={error:"Element not found"};__cb({requestId:%s,data:__result});} else {
  el.scrollIntoView({block:"center",behavior:"smooth"});

  var rect=el.getBoundingClientRect();
  var tx=rect.left+rect.width*(0.3+Math.random()*0.4);
  var ty=rect.top+rect.height*(0.3+Math.random()*0.4);

  var sx=Math.random()*window.innerWidth;
  var sy=Math.random()*window.innerHeight;

  var cx=(sx+tx)/2+(Math.random()-0.5)*200;
  var cy=(sy+ty)/2+(Math.random()-0.5)*200;

  var steps=%d;
  var jitter=%d;
  var baseDelay=%d;

  function bezier(t){
    var u=1-t;
    return {
      x:u*u*sx+2*u*t*cx+t*t*tx,
      y:u*u*sy+2*u*t*cy+t*t*ty
    };
  }

  function dispatchMouse(type,x,y,target){
    var evt=new MouseEvent(type,{
      bubbles:true,cancelable:true,view:window,
      clientX:x,clientY:y,
      screenX:x+window.screenX,screenY:y+window.screenY,
      button:type==='mousedown'||type==='mouseup'||type==='click'?0:-1,
      buttons:type==='mousedown'?1:0
    });
    (target||document.elementFromPoint(x,y)||document.body).dispatchEvent(evt);
  }

  var i=0;
  function moveStep(){
    if(i>=steps){
      dispatchMouse('mouseover',tx,ty,el);
      dispatchMouse('mouseenter',tx,ty,el);
      dispatchMouse('mousemove',tx,ty,el);

      setTimeout(function(){
        dispatchMouse('mousedown',tx,ty,el);
        setTimeout(function(){
          dispatchMouse('mouseup',tx,ty,el);
          dispatchMouse('click',tx,ty,el);
          el.click();

          __cb({requestId:%s,data:{ok:true,tag:el.tagName.toLowerCase(),text:(el.textContent||"").trim().substring(0,80),path:{steps:steps,startX:Math.round(sx),startY:Math.round(sy),endX:Math.round(tx),endY:Math.round(ty)}}});
        }, 20+Math.random()*40);
      }, 30+Math.random()*60);
      return;
    }
    var t=i/steps;
    t=t<0.5?2*t*t:(1-Math.pow(-2*t+2,2)/2);
    var p=bezier(t);
    p.x+=Math.round((Math.random()-0.5)*jitter*2);
    p.y+=Math.round((Math.random()-0.5)*jitter*2);
    dispatchMouse('mousemove',p.x,p.y);
    i++;
    setTimeout(moveStep, baseDelay+Math.random()*baseDelay);
  }
  moveStep();
}`,
		jsonString(requestID),
		steps, jitter, baseDelay,
		jsonString(requestID),
	)

	// cursorClickJS has its own async completion via setTimeout chain
	return fmt.Sprintf(`(function(){%stry{%s}catch(e){__cb({requestId:%s,error:e.message||String(e)});}})();`,
		callbackJS(callbackURL), code, jsonString(requestID))
}

// cursorHoverJS returns JS that simulates a realistic mouse movement to an element
// and hovers over it (without clicking).
func cursorHoverJS(requestID, callbackURL, ref, selector string) string {
	var findCode string
	if ref != "" {
		findCode = fmt.Sprintf(`var el=document.querySelector('[data-nebo-ref=%s]');`, jsonString(ref))
	} else {
		findCode = fmt.Sprintf(`var el=document.querySelector(%s);`, jsonString(selector))
	}

	steps := 12 + rand.Intn(15)
	jitter := 1 + rand.Intn(3)
	baseDelay := 5 + rand.Intn(10)

	code := findCode + fmt.Sprintf(`
if(!el){var __result={error:"Element not found"};__cb({requestId:%s,data:__result});} else {
  el.scrollIntoView({block:"center",behavior:"smooth"});
  var rect=el.getBoundingClientRect();
  var tx=rect.left+rect.width*0.5;
  var ty=rect.top+rect.height*0.5;
  var sx=Math.random()*window.innerWidth;
  var sy=Math.random()*window.innerHeight;
  var cx=(sx+tx)/2+(Math.random()-0.5)*150;
  var cy=(sy+ty)/2+(Math.random()-0.5)*150;
  var steps=%d;
  var jitter=%d;
  var baseDelay=%d;
  function bezier(t){var u=1-t;return{x:u*u*sx+2*u*t*cx+t*t*tx,y:u*u*sy+2*u*t*cy+t*t*ty};}
  function dm(type,x,y,target){
    var evt=new MouseEvent(type,{bubbles:true,cancelable:true,view:window,clientX:x,clientY:y});
    (target||document.elementFromPoint(x,y)||document.body).dispatchEvent(evt);
  }
  var i=0;
  function moveStep(){
    if(i>=steps){
      dm('mouseover',tx,ty,el);
      dm('mouseenter',tx,ty,el);
      dm('mousemove',tx,ty,el);
      __cb({requestId:%s,data:{ok:true,tag:el.tagName.toLowerCase()}});
      return;
    }
    var t=i/steps;
    t=t<0.5?2*t*t:(1-Math.pow(-2*t+2,2)/2);
    var p=bezier(t);
    p.x+=Math.round((Math.random()-0.5)*jitter*2);
    p.y+=Math.round((Math.random()-0.5)*jitter*2);
    dm('mousemove',p.x,p.y);
    i++;
    setTimeout(moveStep, baseDelay+Math.random()*baseDelay);
  }
  moveStep();
}`,
		jsonString(requestID),
		steps, jitter, baseDelay,
		jsonString(requestID),
	)

	return fmt.Sprintf(`(function(){%stry{%s}catch(e){__cb({requestId:%s,error:e.message||String(e)});}})();`,
		callbackJS(callbackURL), code, jsonString(requestID))
}
