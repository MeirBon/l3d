use crate::load::{
    AnimationDescriptor, Channel, LoadOptions, Loader, MeshDescriptor, Method, NodeDescriptor,
    Orthographic, Perspective, SceneDescriptor, SkeletonDescriptor, SkinDescriptor, Target,
};
use crate::load::{CameraDescriptor, Projection};
use crate::mat::{MaterialList, Texture, TextureDescriptor, TextureFormat, TextureSource};
use crate::{LoadError, LoadResult};
use glam::*;
use gltf::{
    animation::util::{MorphTargetWeights, ReadOutputs, Rotations},
    json::animation::{Interpolation, Property},
    mesh::util::{ReadIndices, ReadJoints, ReadTexCoords, ReadWeights},
    scene::Transform,
};

use std::collections::HashMap;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Copy, Clone)]
pub struct GltfLoader {}

impl std::fmt::Display for GltfLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gltf-loader")
    }
}

impl Default for GltfLoader {
    fn default() -> Self {
        Self {}
    }
}

impl Loader for GltfLoader {
    fn name(&self) -> &'static str {
        "glTF loader"
    }

    fn file_extensions(&self) -> Vec<String> {
        vec![String::from("gltf"), String::from("glb")]
    }

    fn load(&self, options: LoadOptions) -> LoadResult {
        let (document, buffers, images) = match {
            match &options.source {
                crate::load::LoadSource::Path(p) => gltf::import(p),
                crate::load::LoadSource::String { source, .. } => gltf::import_slice(source),
            }
        } {
            Ok(g) => g,
            Err(e) => return LoadResult::None(LoadError::Error(Box::new(e))),
        };

        let mut mat_list = MaterialList::new();
        let mut mat_mapping = HashMap::new();

        let load_texture = |texture: &gltf::Texture| {
            let img = images.get(texture.index())?;
            Some(TextureSource::Loaded(Texture::from_bytes(
                img.pixels.as_slice(),
                img.width,
                img.height,
                match img.format {
                    gltf::image::Format::R8 => TextureFormat::R,
                    gltf::image::Format::R8G8 => TextureFormat::RG,
                    gltf::image::Format::R8G8B8 => TextureFormat::RGB,
                    gltf::image::Format::R8G8B8A8 => TextureFormat::RGBA,
                    gltf::image::Format::B8G8R8 => TextureFormat::BGR,
                    gltf::image::Format::B8G8R8A8 => TextureFormat::BGRA,
                    gltf::image::Format::R16 => TextureFormat::R16,
                    gltf::image::Format::R16G16 => TextureFormat::RG16,
                    gltf::image::Format::R16G16B16 => TextureFormat::RGB16,
                    gltf::image::Format::R16G16B16A16 => TextureFormat::RGBA16,
                },
                match img.format {
                    gltf::image::Format::R8 => std::mem::size_of::<u8>(),
                    gltf::image::Format::R8G8 => 2 * std::mem::size_of::<u8>(),
                    gltf::image::Format::R8G8B8 => 3 * std::mem::size_of::<u8>(),
                    gltf::image::Format::R8G8B8A8 => 4 * std::mem::size_of::<u8>(),
                    gltf::image::Format::B8G8R8 => 3 * std::mem::size_of::<u8>(),
                    gltf::image::Format::B8G8R8A8 => 4 * std::mem::size_of::<u8>(),
                    gltf::image::Format::R16 => std::mem::size_of::<u16>(),
                    gltf::image::Format::R16G16 => 2 * std::mem::size_of::<u16>(),
                    gltf::image::Format::R16G16B16 => 3 * std::mem::size_of::<u16>(),
                    gltf::image::Format::R16G16B16A16 => 4 * std::mem::size_of::<u16>(),
                },
            )))
        };

        document.materials().enumerate().for_each(|(i, m)| {
            let pbr = m.pbr_metallic_roughness();

            let index = mat_list.add_with_maps(
                Vec4::from(pbr.base_color_factor()).truncate(),
                pbr.roughness_factor(),
                Vec4::from(pbr.base_color_factor()).truncate(),
                0.0,
                TextureDescriptor {
                    albedo: match pbr.base_color_texture() {
                        Some(tex) => load_texture(&tex.texture()),
                        None => None,
                    },
                    normal: match m.normal_texture() {
                        Some(tex) => load_texture(&tex.texture()),
                        None => None,
                    },
                    metallic_roughness_map:  // TODO: Make sure this works correctly in renderers & modify other loaders to use similar kind of system
                    // The metalness values are sampled from the B channel.
                    // The roughness values are sampled from the G channel.
                    match pbr.metallic_roughness_texture() {
                        Some(tex) => load_texture(&tex.texture()),
                        None => None,
                    },
                    emissive_map: match m.emissive_texture() {
                        Some(tex) => load_texture(&tex.texture()),
                        None => None,
                    },
                    sheen_map: None,
                },
            );

            mat_mapping.insert(m.index().unwrap_or(i), index);
        });

        let meshes: Vec<MeshDescriptor> = document
            .meshes()
            .map(|mesh| {
                let mut tmp_indices = Vec::new();

                let mut vertices: Vec<[f32; 4]> = Vec::new();
                let mut normals: Vec<[f32; 3]> = Vec::new();
                let mut indices: Vec<[u32; 3]> = Vec::new();
                let mut joints: Vec<Vec<[u16; 4]>> = Vec::new();
                let mut weights: Vec<Vec<[f32; 4]>> = Vec::new();
                let mut material_ids: Vec<i32> = Vec::new();
                let mut uvs: Vec<[f32; 2]> = Vec::new();

                mesh.primitives().for_each(|prim| {
                    let reader =
                        prim.reader(|buffer| buffers.get(buffer.index()).map(|b| b.0.as_slice()));
                    if let Some(iter) = reader.read_positions() {
                        for pos in iter {
                            vertices.push([pos[0], pos[1], pos[2], 1.0]);
                        }
                    }

                    if let Some(iter) = reader.read_normals() {
                        for n in iter {
                            normals.push([n[0], n[1], n[2]]);
                        }
                    }

                    if let Some(iter) = reader.read_tex_coords(0) {
                        // TODO: Check whether we need to scale non-float types
                        match iter {
                            ReadTexCoords::U8(iter) => {
                                for uv in iter {
                                    uvs.push([
                                        uv[0] as f32 / u8::MAX as f32,
                                        uv[1] as f32 / u8::MAX as f32,
                                    ]);
                                }
                            }
                            ReadTexCoords::U16(iter) => {
                                for uv in iter {
                                    uvs.push([
                                        uv[0] as f32 / u16::MAX as f32,
                                        uv[1] as f32 / u16::MAX as f32,
                                    ]);
                                }
                            }
                            ReadTexCoords::F32(iter) => {
                                for uv in iter {
                                    uvs.push(uv);
                                }
                            }
                        }
                    }

                    let mut set = 0;
                    loop {
                        let mut stop = true;

                        if let Some(iter) = reader.read_weights(set) {
                            stop = false;
                            weights.push(Vec::new());
                            match iter {
                                ReadWeights::U8(iter) => {
                                    for w in iter {
                                        weights[set as usize].push([
                                            w[0] as f32,
                                            w[1] as f32,
                                            w[2] as f32,
                                            w[3] as f32,
                                        ]);
                                    }
                                }
                                ReadWeights::U16(iter) => {
                                    for w in iter {
                                        weights[set as usize].push([
                                            w[0] as f32,
                                            w[1] as f32,
                                            w[2] as f32,
                                            w[3] as f32,
                                        ]);
                                    }
                                }
                                ReadWeights::F32(iter) => {
                                    for w in iter {
                                        weights[set as usize].push(w);
                                    }
                                }
                            }
                        }

                        if let Some(iter) = reader.read_joints(set) {
                            stop = false;
                            joints.push(Vec::new());
                            match iter {
                                ReadJoints::U8(iter) => {
                                    for j in iter {
                                        joints[set as usize].push([
                                            j[0] as u16,
                                            j[1] as u16,
                                            j[2] as u16,
                                            j[3] as u16,
                                        ]);
                                    }
                                }
                                ReadJoints::U16(iter) => {
                                    for j in iter {
                                        joints[set as usize].push(j);
                                    }
                                }
                            }
                        }

                        if stop {
                            break;
                        }

                        set += 1;
                    }

                    tmp_indices.clear();
                    if let Some(iter) = reader.read_indices() {
                        match iter {
                            ReadIndices::U8(iter) => {
                                for idx in iter {
                                    let idx = idx as u32;
                                    tmp_indices.push(idx as u32);
                                }
                            }
                            ReadIndices::U16(iter) => {
                                for idx in iter {
                                    let idx = idx as u32;
                                    tmp_indices.push(idx as u32);
                                }
                            }
                            ReadIndices::U32(iter) => {
                                for idx in iter {
                                    let idx = idx;
                                    tmp_indices.push(idx);
                                }
                            }
                        }
                    }

                    match prim.mode() {
                        gltf::mesh::Mode::Points => unimplemented!(),
                        gltf::mesh::Mode::Lines => unimplemented!(),
                        gltf::mesh::Mode::LineLoop => unimplemented!(),
                        gltf::mesh::Mode::LineStrip => unimplemented!(),
                        gltf::mesh::Mode::Triangles => {
                            // Nothing to do
                        }
                        gltf::mesh::Mode::TriangleStrip => {
                            let strip = tmp_indices.clone();
                            tmp_indices.clear();
                            for p in 2..strip.len() {
                                tmp_indices.push(strip[p - 2]);
                                tmp_indices.push(strip[p - 1]);
                                tmp_indices.push(strip[p]);
                            }
                        }
                        gltf::mesh::Mode::TriangleFan => {
                            let fan = tmp_indices.clone();
                            tmp_indices.clear();
                            for p in 2..fan.len() {
                                tmp_indices.push(fan[0]);
                                tmp_indices.push(fan[p - 1]);
                                tmp_indices.push(fan[p]);
                            }
                        }
                    }

                    let mat_id = *mat_mapping
                        .get(&prim.material().index().unwrap_or(0))
                        .unwrap_or(&0) as u32;

                    let iter = tmp_indices.chunks(3);
                    let length = iter.len();
                    for ids in iter {
                        indices.push([
                            ids[0],
                            ids[1.min(ids.len() - 1)],
                            ids[2.min(ids.len() - 1)],
                        ]);
                    }

                    material_ids.resize(material_ids.len() + length * 3, mat_id as i32);
                });

                MeshDescriptor::new_indexed(
                    indices,
                    vertices,
                    normals,
                    uvs,
                    match (joints.is_empty(), weights.is_empty()) {
                        (false, false) => Some(SkeletonDescriptor { joints, weights }),
                        _ => None,
                    },
                    material_ids,
                    None,
                    Some(String::from(mesh.name().unwrap_or(""))),
                )
            })
            .collect();

        let mut animations: Vec<AnimationDescriptor> = Vec::new();
        for anim in document.animations() {
            let channels: Vec<(u32, Channel)> = anim
                .channels()
                .map(|c| {
                    let mut channel = Channel::default();
                    let reader =
                        c.reader(|buffer| buffers.get(buffer.index()).map(|b| b.0.as_slice()));

                    channel.sampler = match c.sampler().interpolation() {
                        Interpolation::Linear => Method::Linear,
                        Interpolation::Step => Method::Step,
                        Interpolation::CubicSpline => Method::Spline,
                    };

                    let target = c.target();
                    let target_node_id = target.node().index();

                    channel.targets.push(match target.property() {
                        Property::Translation => Target::Translation,
                        Property::Rotation => Target::Rotation,
                        Property::Scale => Target::Scale,
                        Property::MorphTargetWeights => Target::MorphWeights,
                    });

                    if let Some(inputs) = reader.read_inputs() {
                        inputs.for_each(|input| {
                            channel.key_frames.push(input);
                        });
                    }

                    if let Some(outputs) = reader.read_outputs() {
                        match outputs {
                            ReadOutputs::Translations(t) => {
                                t.for_each(|t| {
                                    channel.vec3s.push(t);
                                });
                            }
                            ReadOutputs::Rotations(r) => match r {
                                Rotations::I8(r) => {
                                    r.for_each(|r| {
                                        let r = [
                                            r[0] as f32 / (std::i8::MAX) as f32,
                                            r[1] as f32 / (std::i8::MAX) as f32,
                                            r[2] as f32 / (std::i8::MAX) as f32,
                                            r[3] as f32 / (std::i8::MAX) as f32,
                                        ];
                                        channel.rotations.push([r[0], r[1], r[2], r[3]]);
                                    });
                                }
                                Rotations::U8(r) => {
                                    r.for_each(|r| {
                                        let r = [
                                            r[0] as f32 / (std::u8::MAX) as f32,
                                            r[1] as f32 / (std::u8::MAX) as f32,
                                            r[2] as f32 / (std::u8::MAX) as f32,
                                            r[3] as f32 / (std::u8::MAX) as f32,
                                        ];
                                        channel.rotations.push(r);
                                    });
                                }
                                Rotations::I16(r) => {
                                    r.for_each(|r| {
                                        let r = [
                                            r[0] as f32 / (std::i16::MAX) as f32,
                                            r[1] as f32 / (std::i16::MAX) as f32,
                                            r[2] as f32 / (std::i16::MAX) as f32,
                                            r[3] as f32 / (std::i16::MAX) as f32,
                                        ];
                                        channel.rotations.push(r);
                                    });
                                }
                                Rotations::U16(r) => {
                                    r.for_each(|r| {
                                        let r = [
                                            r[0] as f32 / (std::u16::MAX) as f32,
                                            r[1] as f32 / (std::u16::MAX) as f32,
                                            r[2] as f32 / (std::u16::MAX) as f32,
                                            r[3] as f32 / (std::u16::MAX) as f32,
                                        ];
                                        channel.rotations.push(r);
                                    });
                                }
                                Rotations::F32(r) => {
                                    r.for_each(|r| {
                                        channel.rotations.push(r);
                                    });
                                }
                            },
                            ReadOutputs::Scales(s) => {
                                s.for_each(|s| {
                                    channel.vec3s.push(s);
                                });
                            }
                            ReadOutputs::MorphTargetWeights(m) => match m {
                                MorphTargetWeights::I8(m) => {
                                    m.for_each(|m| {
                                        let m = m as f32 / std::i8::MAX as f32;
                                        channel.weights.push(m);
                                    });
                                }
                                MorphTargetWeights::U8(m) => {
                                    m.for_each(|m| {
                                        let m = m as f32 / std::u8::MAX as f32;
                                        channel.weights.push(m);
                                    });
                                }
                                MorphTargetWeights::I16(m) => {
                                    m.for_each(|m| {
                                        let m = m as f32 / std::i16::MAX as f32;
                                        channel.weights.push(m);
                                    });
                                }
                                MorphTargetWeights::U16(m) => {
                                    m.for_each(|m| {
                                        let m = m as f32 / std::u16::MAX as f32;
                                        channel.weights.push(m);
                                    });
                                }
                                MorphTargetWeights::F32(m) => {
                                    m.for_each(|m| {
                                        channel.weights.push(m);
                                    });
                                }
                            },
                        }
                    }

                    channel.duration = *channel.key_frames.last().unwrap();

                    (target_node_id as u32, channel)
                })
                .collect();

            animations.push(AnimationDescriptor {
                name: anim.name().unwrap_or("").to_string(),
                // TODO
                //affected_roots: nodes.root_nodes(),
                channels,
            });
        }

        let mut nodes = vec![];
        for scene in document.scenes().into_iter() {
            // Iterate over root nodes.
            for node in scene.nodes() {
                nodes.push(load_node(&document, &buffers, &node));
            }
        }

        let descriptor = SceneDescriptor {
            materials: mat_list,
            meshes,
            nodes,
            animations,
        };

        LoadResult::Scene(descriptor)
    }
}

fn load_node(
    gltf: &gltf::Document,
    gltf_buffers: &[gltf::buffer::Data],
    node: &gltf::Node,
) -> NodeDescriptor {
    let (scale, rotation, translation): ([f32; 3], [f32; 4], [f32; 3]) = match node.transform() {
        Transform::Matrix { matrix } => {
            let (scale, rotation, translation) =
                Mat4::from_cols_array_2d(&matrix).to_scale_rotation_translation();

            (scale.into(), rotation.into(), translation.into())
        }
        Transform::Decomposed {
            translation,
            rotation,
            scale,
        } => (scale, rotation, translation),
    };

    let mut node_meshes: Vec<u32> = Vec::new();
    if let Some(mesh) = node.mesh() {
        node_meshes.push(mesh.index() as u32);
    }

    let maybe_skin = node.skin().map(|s| {
        let name = s.name().map(|n| n.into()).unwrap_or_default();
        let joint_nodes = s
            .joints()
            .map(|joint_node| joint_node.index() as u32)
            .collect();

        let mut inverse_bind_matrices = vec![];
        let reader = s.reader(|buffer| gltf_buffers.get(buffer.index()).map(|d| d.0.as_slice()));
        if let Some(ibm) = reader.read_inverse_bind_matrices() {
            ibm.for_each(|m| {
                let mat = Mat4::from_cols_array_2d(&m);
                inverse_bind_matrices.push(mat.to_cols_array());
            });
        }

        SkinDescriptor {
            name,
            inverse_bind_matrices,
            joint_nodes,
        }
    });

    let mut child_nodes = vec![];
    if node.children().len() > 0 {
        child_nodes.reserve(node.children().len());
        for child in node.children() {
            child_nodes.push(load_node(gltf, gltf_buffers, &child));
        }
    }

    let camera = if let Some(camera) = node.camera() {
        let projection = match camera.projection() {
            gltf::camera::Projection::Orthographic(o) => Projection::Orthographic(Orthographic {
                x_mag: o.xmag(),
                y_mag: o.ymag(),
                z_near: o.znear(),
                z_far: o.zfar(),
            }),
            gltf::camera::Projection::Perspective(p) => Projection::Perspective(Perspective {
                aspect_ratio: p.aspect_ratio(),
                y_fov: p.yfov(),
                z_near: p.znear(),
                z_far: p.zfar(),
            }),
        };

        Some(CameraDescriptor { projection })
    } else {
        None
    };

    NodeDescriptor {
        name: node.name().map(|n| n.into()).unwrap_or_default(),
        child_nodes,
        camera,

        translation,
        rotation,
        scale,

        meshes: node_meshes,
        skin: maybe_skin,
        weights: node.weights().map(|w| w.to_vec()).unwrap_or_default(),

        id: node.index() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::Loader;
    use crate::load::gltf::GltfLoader;
    use crate::load::*;
    use core::panic;
    use rtbvh::Aabb;
    use std::path::PathBuf;

    #[test]
    fn load_gltf_works() {
        let loader = GltfLoader::default();
        let gltf = loader.load(LoadOptions {
            source: LoadSource::Path(PathBuf::from("assets/CesiumMan.gltf")),
            ..Default::default()
        });

        let scene = match gltf {
            crate::LoadResult::Mesh(_) => panic!("glTF loader should only return scenes"),
            crate::LoadResult::Scene(s) => s,
            crate::LoadResult::None(_) => panic!("glTF loader should successfully load scenes"),
        };

        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.materials.len_textures(), 1);

        let m = &scene.meshes[0];
        assert_eq!(m.vertices.len(), 14016);
        assert_eq!(m.vertices.len(), m.normals.len());
        assert_eq!(m.normals.len(), m.uvs.len());
        assert_eq!(m.uvs.len(), m.tangents.len());
        assert_eq!(m.tangents.len(), m.material_ids.len());

        assert_eq!(m.meshes.len(), 1);
        assert!(m.skeleton.is_some());

        // Bounds should be correct
        let mut aabb: Aabb<()> = Aabb::new();
        for v in m.vertices.iter() {
            aabb.grow(vec3(v[0], v[1], v[2]));
        }
        for i in 0..3 {
            assert!((aabb.min[i] - m.bounds.min[i]).abs() < f32::EPSILON);
            assert!((aabb.max[i] - m.bounds.max[i]).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn load_gltf_source_works() {
        let loader = GltfLoader::default();
        let gltf = loader.load(LoadOptions {
            source: LoadSource::String {
                source: include_bytes!("../../assets/CesiumMan.glb"),
                extension: "gtlf",
                basedir: "assets",
            },
            ..Default::default()
        });

        let scene = match gltf {
            crate::LoadResult::Mesh(_) => panic!("glTF loader should only return scenes"),
            crate::LoadResult::Scene(s) => s,
            crate::LoadResult::None(_) => {
                panic!("glTF loader should successfully load scenes from strings")
            }
        };

        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.materials.len_textures(), 1);

        let m = &scene.meshes[0];
        assert_eq!(m.vertices.len(), 14016);
        assert_eq!(m.vertices.len(), m.normals.len());
        assert_eq!(m.normals.len(), m.uvs.len());
        assert_eq!(m.uvs.len(), m.tangents.len());
        assert_eq!(m.tangents.len(), m.material_ids.len());

        assert_eq!(m.meshes.len(), 1);
        assert!(m.skeleton.is_some());

        // Bounds should be correct
        let mut aabb: Aabb<()> = Aabb::new();
        for v in m.vertices.iter() {
            aabb.grow(vec3(v[0], v[1], v[2]));
        }
        for i in 0..3 {
            assert!((aabb.min[i] - m.bounds.min[i]).abs() < f32::EPSILON);
            assert!((aabb.max[i] - m.bounds.max[i]).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn load_glb_works() {
        let loader = GltfLoader::default();
        let gltf = loader.load(LoadOptions {
            source: LoadSource::Path(PathBuf::from("assets/CesiumMan.glb")),
            ..Default::default()
        });

        let scene = match gltf {
            crate::LoadResult::Mesh(_) => panic!("glTF loader should only return scenes"),
            crate::LoadResult::Scene(s) => s,
            crate::LoadResult::None(_) => panic!("glTF loader should successfully load scenes"),
        };

        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.materials.len_textures(), 1);

        let m = &scene.meshes[0];
        assert_eq!(m.vertices.len(), 14016);
        assert_eq!(m.vertices.len(), m.normals.len());
        assert_eq!(m.normals.len(), m.uvs.len());
        assert_eq!(m.uvs.len(), m.tangents.len());
        assert_eq!(m.tangents.len(), m.material_ids.len());

        assert_eq!(m.meshes.len(), 1);
        assert!(m.skeleton.is_some());

        // Bounds should be correct
        let mut aabb: Aabb<()> = Aabb::new();
        for v in m.vertices.iter() {
            aabb.grow(vec3(v[0], v[1], v[2]));
        }
        for i in 0..3 {
            assert!((aabb.min[i] - m.bounds.min[i]).abs() < f32::EPSILON);
            assert!((aabb.max[i] - m.bounds.max[i]).abs() < f32::EPSILON);
        }
    }
}
