use glam::{Mat4, Vec3, Vec3A};
use rend3_types::{Camera, CameraProjection, Handedness};

use crate::managers::{CameraState, InternalDirectionalLight};

pub(super) fn shadow_camera(l: &InternalDirectionalLight, user_camera: &CameraState) -> CameraState {
    let camera_location = user_camera.location();

    let shadow_texel_size = l.inner.distance / l.inner.resolution as f32;

    let look_at = match user_camera.handedness() {
        /* Handedness::Left => Mat4::look_at_lh, REMOVED IN GLAM */
        Handedness::Left => glam::camera::lh::view::look_at_mat4,
        /* Handedness::Right => Mat4::look_at_rh, REMOVED IN GLAM */
        Handedness::Right => glam::camera::rh::view::look_at_mat4,
    };

    let origin_view = look_at(Vec3::ZERO, l.inner.direction, Vec3::Y);
    let camera_origin_view = origin_view.transform_point3(camera_location);

    let offset = camera_origin_view.truncate() % shadow_texel_size;
    let shadow_location = camera_origin_view - Vec3::from((offset, 0.0));

    let inv_origin_view = origin_view.inverse();
    let new_shadow_location = inv_origin_view.transform_point3(shadow_location);

    CameraState::new(
        Camera {
            projection: CameraProjection::Orthographic { size: Vec3A::splat(l.inner.distance) },
            view: look_at(new_shadow_location, new_shadow_location + l.inner.direction, Vec3::Y),
        },
        user_camera.handedness(),
        None,
    )
}
