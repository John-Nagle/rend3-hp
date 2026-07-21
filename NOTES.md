# Development notes
John Nagle
nagle@animats.com

Reviving Rend3 after its abandonment.

## 2024-10-12

Changes to WGPU have broken Rend3. The worst change is

https://github.com/gfx-rs/wgpu/pull/5884

which changes the lifetime of "RenderPass" and Encoder to 'static

which generates this problem:

https://users.rust-lang.org/t/unexpected-borrowed-data-escapes-outside-of-closure-for-generic/119501/4

RenderPass and Controller ownership is currently manipulated by unsafe code. 
That needs to be figured out. 

###Trouble spot: take_rpass
  
https://github.com/BVE-Reborn/rend3/blob/d088a841b0469d07d5a7ff3f4d784e97b4a194d5/rend3/src/graph/encpass.rs#L74
  
    pub(super) enum RenderGraphEncoderOrPassInner<'a, 'pass> {
        Encoder(&'a mut CommandEncoder),
        RenderPass(&'a mut RenderPass<'pass>),
        #[default]
        None,
    }   
  
    pub struct RenderGraphEncoderOrPass<'a, 'pass>(pub(super) RenderGraphEncoderOrPassInner<'a, 'pass>);
    ...
    pub fn take_rpass(&mut self, _handle: DeclaredDependency<RenderPassHandle>) -> &'a mut RenderPass<'pass> {
    match mem::take(&mut self.0) {
        
This in itself seems legit, i.e. safe. Except that it's taking a mutable reference, not the object itself.
There can only be one such mutable reference. Where does this come from?
  
Around here:
  
  https://github.com/BVE-Reborn/rend3/blob/d088a841b0469d07d5a7ff3f4d784e97b4a194d5/rend3/src/graph/graph.rs#L451
  
where there is unsafe code to seemingly allow two mutable references to the same object to exist simultaneously.
  
    // SAFETY: There is no active renderpass to borrow this. This reference lasts for the duration of
    // the call to exec.
    None => RenderGraphEncoderOrPassInner::Encoder(unsafe { &mut *encoder_cell.get() }),
      
Note that this is not an assertion that the code here is safe.
It is an unchecked constraint on the rest of the program.
  
Why is this not done with a RefCell and .borrow_mut?
  
Other than the mess around RenderPass, there's not that much unsafe code in rend3.
This might be fixable.
  
Maybe if we pass around something that has a RefCell<RenderGraphEncoderOrPassInner>
to everybody who needs that...
But that encapsulates a mutable reference. Who actually *owns* the thing?
Question: if we put RenderPass and Encoder inside an Rc<RefCell<>>, will that work?
Or will we panic at borrows?
Worth a try.

## 2024-10-13

Who actually owns RenderPass and Encoder?

    RenderGraphEncoderOrPassInner holds an &mut of CommandEncoder or RenderPass

Who actually creates RenderGraphEncoderOrPassInner?
* Created in graph.rs execute

  https://github.com/BVE-Reborn/rend3/blob/d088a841b0469d07d5a7ff3f4d784e97b4a194d5/rend3/src/graph/graph.rs#L451

      RenderGraphEncoderOrPassInner::RenderPass(rpass)

  https://github.com/BVE-Reborn/rend3/blob/d088a841b0469d07d5a7ff3f4d784e97b4a194d5/rend3/src/graph/graph.rs#L455

       None => RenderGraphEncoderOrPassInner::Encoder(unsafe { &mut *encoder_cell.get() }),
 
  Unsafe code. Is this really necessary?
  
  https://github.com/BVE-Reborn/rend3/blob/d088a841b0469d07d5a7ff3f4d784e97b4a194d5/rend3/src/graph/graph.rs#L484
  
  Same situation.
  
  That's a get from an UnsafeCell. That's probably what should be a RefCell.
  The UnsafeCell actually owns the encoder. 
  Who owns the RenderPass? 
  The caller of 
      create_rpass_from_desc
  
### First cut at design:

      create_rpass_from_desc

returns an Rc<RefCell<RenderPass>>.

At line 391 in graph.rs,
  encoder_cell becomes an Rc<RefCell>> of a new command encoder.
  
Similarly for rpass_temps_cell.

RenderGraphEncoderOrPassInner must hold an Rc<RefCell<>> for each item.

So where do we do the borrows?

At the unsafe points? Probably.

What about take_rpass? 
Do the borrow at self.internal.render in lib.rs? 

This will probably all compile and it's all safe code. If the comments about safety are true,
the borrow_mut calls should not fail. We will see.

## 2024-10-14

Fix all other compile errors from Winit changes. [DONE]

## 2024-10-16

Completed fixing Winit problems, with event loop disabled.
Time to do the above changes to Rend3.

Most of it now compiles.

Trouble at line 507 of graph.rs

## 2026-07-09

Back working on Rend3. Works with WGPU 24. Even does the Bistro scene. 
But the transparent windows in the Bistro scene come through as transparent areas in the output render
with the version of the Bistro .glb file we found. And the reddish street light glasses became fully transparent. 
Need to check transparency handling on known tests.

It looks like translucency does not work at all in the scene-viewer demos. Look at how assets are loaded.
Rend3 can do transparency but the GLTF loader may not be testing for images that need it.

## 2026-07-18

Looking at the Bistro scene in Blender, there's no translucency in those windows. 
They are showing a reflection of the default Blender PBR scene.
These are CHANNEL_PACKED images, but what's in the alpha channel?
The windows are opaque. The only BLEND texture in the Bistro scene is the speedometer of the motor scooter, per the .gltf file.
- So this is not a translucency bug at all.
- It's an opaque item with an alpha channel cutting out things that should not show. What's that supposed to do, anyway?
- Checking ivy, "Foliage_Ivy_leaf_a"
  - Ivy has MASK mode in glTF, which is correct. 
  - Ivy leaf image has 100% alpha in leaf area, 0% alpha in cutout, which is correct. Color in cutout is 0,0,0 alpha 0.
  - Ivy should have rendered correctly. But it's showing in the cargo test image bistro-failure.png as background empty squares.
  - Some other leaf-type objects have an alpha cutoff value, but this does not, so it should default to 0.5 and work.
  - Works OK in scene-viewer mode. Broken in cargo test mode.
- Checking MenuSign
  - MenuSign_02_Glass" has an alpha cutoff of 1 in MASK mode, which probably won't work.
- Other than MasterForgeMetal, MASK mode is used only for foliage.

Asked Github Copilot:

What is different about the way scene_viewer handles alpha channels and the way scene_viewer unit tests handle alpha channels?

I'll search for how alpha channels are handled in the scene_viewer and its unit tests.

Let me search for the scene_viewer module and its tests more specifically:

Let me look at the scene_viewer implementation and its tests more closely:

Now I can see the key difference! Let me examine the clear_color settings more closely:

Based on my examination of the code, I found the key difference in how scene_viewer handles alpha channels:
Alpha Channel Handling Difference

In scene_viewer's normal rendering (line 690):
Rust

clear_color: glam::Vec4::new(0.0, 0.0, 0.0, 1.0),

The clear color has an alpha channel of 1.0 (fully opaque).

In scene_viewer's unit tests (line 43 in tests.rs):
Rust

wgpu::TextureFormat::Rgba8UnormSrgb,

The test uses Rgba8UnormSrgb format, which supports the full alpha channel, but the test creates a texture without any specific alpha handling.

The practical difference:

    Normal rendering: Scene_viewer clears the frame with a fully opaque black background (0, 0, 0, 1), meaning the alpha channel is completely opaque.
    Unit tests: The tests render to an Rgba8UnormSrgb texture that supports alpha, but there's no explicit alpha blending or transparency handling configured in the test environment—it's a direct render to texture without the window/display compositing that might require alpha.

This means scene_viewer in normal operation doesn't support transparent backgrounds (alpha is always 1.0), while the test infrastructure could theoretically support transparent pixels in the output texture, though the tests themselves don't exercise any transparency features.

 --
So, tried setting clear_color: glam::Vec4::new(0.0, 0.0, 0.0, 1.0), in "runner". Didn't help.
Copilot is helpful but not really correct.

2026-07-20

Still haven't figured out the unit test problem. But egui 0.30 is out, so will try that integration again.



  
  
